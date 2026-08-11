//! The album-database worker thread: owns the SQLite connection to
//! `albums.db`, serving reads/writes requested by the main thread and
//! polling for external changes (`album-tool`, a GUI tool, hand SQL). See
//! `spec.md` §6 for the full design.
//!
//! Not unit-tested: the worker loop, `Worker::poll`, and the `data_version`
//! polling all need a real (or at least real-ish) SQLite connection and a
//! live timer, which is what running the app and following the manual
//! checklist (`plan.md` step 4) is for. `albumdb`'s own test suite already
//! covers the storage calls this module wraps.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use albumdb::{AlbumFlag, Clock, Connection, SystemClock};
use anyhow::{Context, anyhow};
use log::warn;

use crate::menu::year_of;

const POLL_INTERVAL: Duration = Duration::from_secs(3);

/// What the main thread asks of the worker.
#[allow(clippy::enum_variant_names)] // `Set*` matches spec §7.2 exactly; not worth the rename.
pub enum AlbumRequest {
    /// The playing album changed (or there is none).
    SetCurrentAlbum(Option<String>),
    /// `u8`, not `Option<u8>` — the menu cannot clear a rating (spec §4.3).
    SetRating { path: String, rating: u8 },
    SetFlag {
        path: String,
        flag: AlbumFlag,
        value: bool,
    },
}

/// The album facts the menu needs, extracted from a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumInfo {
    pub year: Option<String>,
    /// `Option`, unlike `SetRating`'s `u8`: unrated is a state we display
    /// (1470 albums today) even though the menu cannot set it.
    pub rating: Option<u8>,
    pub relisten: bool,
    pub classical: bool,
    pub spoken: bool,
    pub soundtrack: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlbumState {
    /// Nothing playing, or a library-root file with no parent directory.
    NoAlbum,
    /// The database could not be read.
    Unavailable,
    /// Readable, but no row for this path.
    NotFound,
    Found(AlbumInfo),
}

/// One message to the event loop: the current state, plus any error from the
/// most recent write. `write_error` is cleared by the next successful write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumEvent {
    pub state: AlbumState,
    pub write_error: Option<String>,
}

fn album_info(album: &albumdb::Album) -> AlbumInfo {
    AlbumInfo {
        year: year_of(album.original_date.as_deref(), album.date.as_deref()).map(str::to_string),
        rating: album.rating.and_then(|r| u8::try_from(r).ok()),
        relisten: album.needs_relisten,
        classical: album.is_classical,
        spoken: album.is_spoken,
        soundtrack: album.is_soundtrack,
    }
}

fn data_version(conn: &Connection) -> anyhow::Result<i64> {
    conn.pragma_query_value(None, "data_version", |row| row.get(0))
        .context("failed to read data_version")
}

/// Owns the SQLite connection from inside this thread — `rusqlite::Connection`
/// is `Send` but not `Sync`, so it must never cross a thread boundary.
struct Worker {
    db_path: PathBuf,
    conn: Option<Connection>,
    /// The path the poll should watch — set by `AlbumRequest::SetCurrentAlbum`.
    current_path: Option<String>,
    /// `None` until a connection has read the pragma at least once, so the
    /// first poll after (re)opening a connection always re-reads the watched
    /// row rather than assuming nothing changed — `data_version` is a
    /// per-connection sequence, not a persistent one (spec §6.3).
    last_data_version: Option<i64>,
    /// The last `AlbumState` actually sent, so `poll` can dedupe against an
    /// unrelated write elsewhere in the database.
    last_state: AlbumState,
    /// Sticky until the next successful write — carried on every emitted
    /// `AlbumEvent`, not just the one from the write that caused it.
    write_error: Option<String>,
}

impl Worker {
    fn new(db_path: PathBuf) -> Self {
        Worker {
            db_path,
            conn: None,
            current_path: None,
            last_data_version: None,
            last_state: AlbumState::NoAlbum,
            write_error: None,
        }
    }

    /// Opens the connection if it isn't already open, using `open_existing`
    /// so this thread can never create or migrate `albums.db` (spec §6.4).
    fn ensure_conn(&mut self) -> anyhow::Result<&Connection> {
        if self.conn.is_none() {
            self.conn = Some(albumdb::open_existing(&self.db_path)?);
        }
        Ok(self.conn.as_ref().expect("just set above"))
    }

    fn handle(&mut self, request: AlbumRequest, on_event: &impl Fn(AlbumEvent)) {
        match request {
            AlbumRequest::SetCurrentAlbum(path) => {
                self.current_path = path;
                self.emit_current_state(on_event);
            }
            AlbumRequest::SetRating { path, rating } => {
                let result = self.write(&path, |conn, album_id| {
                    albumdb::set_rating(conn, album_id, Some(rating), SystemClock.now())
                });
                self.record_write_result(result);
                self.emit_current_state(on_event);
            }
            AlbumRequest::SetFlag { path, flag, value } => {
                let result = self.write(&path, |conn, album_id| {
                    albumdb::set_flag(conn, album_id, flag, value)
                });
                self.record_write_result(result);
                self.emit_current_state(on_event);
            }
        }
    }

    /// Looks up `path`, then runs `op` against its `album_id` on the shared
    /// connection. A path with no row is reported as a write failure rather
    /// than silently doing nothing — it means the row vanished between the
    /// menu being built and the click landing.
    fn write(
        &mut self,
        path: &str,
        op: impl FnOnce(&Connection, i64) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        let conn = self.ensure_conn()?;
        let album = albumdb::find_by_path(conn, path)?
            .ok_or_else(|| anyhow!("no album at path {path:?}"))?;
        op(conn, album.album_id)
    }

    fn record_write_result(&mut self, result: anyhow::Result<()>) {
        match result {
            Ok(()) => self.write_error = None,
            Err(e) => {
                warn!("album database write failed: {e:#}");
                self.write_error = Some(e.to_string());
            }
        }
    }

    /// Reads and emits the state for `current_path`, unconditionally — used
    /// after a direct request (a track change or a menu click), where the
    /// caller expects to see the result of what it just did, not wait for
    /// the next poll to notice.
    fn emit_current_state(&mut self, on_event: &impl Fn(AlbumEvent)) {
        let state = self.read_state();
        self.last_state = state.clone();
        on_event(AlbumEvent {
            state,
            write_error: self.write_error.clone(),
        });
    }

    /// Emits only if `state` differs from the last one sent — used by
    /// `poll`, so a write elsewhere in the database doesn't churn the menu
    /// (spec §6.3).
    fn emit_if_changed(&mut self, state: AlbumState, on_event: &impl Fn(AlbumEvent)) {
        if state == self.last_state {
            return;
        }
        self.last_state = state.clone();
        on_event(AlbumEvent {
            state,
            write_error: self.write_error.clone(),
        });
    }

    fn read_state(&mut self) -> AlbumState {
        let Some(path) = self.current_path.clone() else {
            return AlbumState::NoAlbum;
        };
        let conn = match self.ensure_conn() {
            Ok(conn) => conn,
            Err(e) => {
                warn!("album database unavailable: {e:#}");
                return AlbumState::Unavailable;
            }
        };
        match albumdb::find_by_path(conn, &path) {
            Ok(Some(album)) => AlbumState::Found(album_info(&album)),
            Ok(None) => AlbumState::NotFound,
            Err(e) => {
                warn!("failed to read album row for {path:?}: {e:#}");
                self.conn = None;
                AlbumState::Unavailable
            }
        }
    }

    /// Per spec §6.3: compares `PRAGMA data_version` to the last-seen value;
    /// unchanged means nothing to do, changed means re-read the watched row
    /// and emit only if the resulting state differs. A pragma error drops
    /// the connection so the next tick retries against a fresh one.
    fn poll(&mut self, on_event: &impl Fn(AlbumEvent)) {
        let conn = match self.ensure_conn() {
            Ok(conn) => conn,
            Err(e) => {
                warn!("album database unavailable: {e:#}");
                self.emit_if_changed(AlbumState::Unavailable, on_event);
                return;
            }
        };
        match data_version(conn) {
            Ok(version) => {
                if self.last_data_version == Some(version) {
                    return;
                }
                self.last_data_version = Some(version);
                let state = self.read_state();
                self.emit_if_changed(state, on_event);
            }
            Err(e) => {
                warn!("data_version pragma failed, dropping connection: {e:#}");
                self.conn = None;
                self.last_data_version = None;
                self.emit_if_changed(AlbumState::Unavailable, on_event);
            }
        }
    }
}

/// Runs forever: serves requests from `rx` and polls for external changes
/// every `POLL_INTERVAL`, pushing state to `on_event`. Mirrors `mpd::run`'s
/// shape. Exits when `rx` disconnects — i.e. when the event loop is dropped
/// at quit — so no explicit shutdown message is needed.
///
/// The timeout is computed from a `next_poll` deadline rather than passed as
/// a bare `POLL_INTERVAL`, so a burst of requests can't starve the poll.
pub fn run(db_path: PathBuf, rx: Receiver<AlbumRequest>, on_event: impl Fn(AlbumEvent)) {
    let mut worker = Worker::new(db_path);
    let mut next_poll = Instant::now() + POLL_INTERVAL;
    loop {
        let timeout = next_poll.saturating_duration_since(Instant::now());
        match rx.recv_timeout(timeout) {
            Ok(request) => worker.handle(request, &on_event),
            Err(RecvTimeoutError::Timeout) => {
                worker.poll(&on_event);
                next_poll = Instant::now() + POLL_INTERVAL;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}
