use std::path::Path;

use albumdb::AlbumFlag;
use anyhow::Result;
use mpd_client::responses::PlayState;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use tray_icon::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};

use crate::URL_SCHEME;
use crate::album::{AlbumInfo, AlbumState};
use crate::mpd::PlayerSnapshot;

pub const PLAY_PAUSE_ID: &str = "play_pause";
pub const NEXT_ID: &str = "next";
pub const CLEAR_ID: &str = "clear";
pub const QUIT_ID: &str = "quit";

/// A queue entry reduced to primitives — no `mpd_client` types, so the model
/// layer is unit-testable without a live connection. Extracted at the
/// `main.rs` call site from `SongInQueue`.
pub struct QueueEntry {
    pub url: String,
    pub album: Option<String>,
    pub title: Option<String>,
    pub artists: Vec<String>,
    pub id: u64,
}

/// A song's album identity: two queue entries are the same album
/// iff they share a parent directory. Chosen over the `album`/`albumartist`
/// tags, which merge albums with the same name (like "Peter Gabriel" self titled!)
pub fn album_key(url: &str) -> Option<&str> {
    url.rsplit_once('/').map(|(dir, _)| dir)
}

/// A library-root file (no parent directory) never shares an album with
/// anything, including another root-level file.
fn same_album(a: &str, b: &str) -> bool {
    matches!((album_key(a), album_key(b)), (Some(x), Some(y)) if x == y)
}

/// The maximal contiguous range of queue positions, containing `current_pos`,
/// that share `current_pos`'s album. Scans outward from
/// `current_pos` only, so cost is O(album length), not O(queue). Returns
/// `None` for an empty queue or an out-of-bounds position.
fn album_run(queue: &[QueueEntry], current_pos: usize) -> Option<(usize, usize)> {
    let current = queue.get(current_pos)?;

    let mut start = current_pos;
    while start > 0 && same_album(&queue[start - 1].url, &current.url) {
        start -= 1;
    }

    let mut end = current_pos;
    while end + 1 < queue.len() && same_album(&queue[end + 1].url, &current.url) {
        end += 1;
    }

    Some((start, end))
}

/// `random` hides album nav because MPD plays a random *order*
/// while leaving queue order untouched, making queue-order jumps meaningless.
/// run_len check means we don't see track-only playlists as lots of albums.
fn show_album_nav(random: bool, run_len: usize) -> bool {
    !random && run_len >= ALBUM_NAV_MIN_RUN
}

const ALBUM_NAV_MIN_RUN: usize = 5;

/// An album-navigation jump target. `label` is the complete row
/// text, so `render` and click dispatch never branch on which of the prev-item
/// states produced it.
#[derive(Debug, PartialEq, Eq)]
pub struct AlbumJump {
    pub label: String,
    pub song_id: u64,
}

fn directory_basename(url: &str) -> Option<&str> {
    let dir = album_key(url)?;
    Some(dir.rsplit('/').next().unwrap_or(dir))
}

/// Untruncated — `model` truncates every finished dropdown label in one
/// place, so they all share one limit.
fn album_display_name(entry: &QueueEntry) -> String {
    entry
        .album
        .clone()
        .or_else(|| directory_basename(&entry.url).map(str::to_string))
        .unwrap_or_else(|| entry.url.clone())
}

/// Next-album target: the run immediately following the current
/// one, if any. `None` when there's no current run (empty/out-of-bounds
/// queue position) or the current run is the last one.
fn next_album_jump(queue: &[QueueEntry], current_pos: usize) -> Option<AlbumJump> {
    let (_, end) = album_run(queue, current_pos)?;
    let entry = queue.get(end + 1)?;
    Some(AlbumJump {
        label: format!("Next Album: {}", album_display_name(entry)),
        song_id: entry.id,
    })
}

/// Prev/Start-of-Album target: If on the first track of an album, jump to previous album.
/// If on the second or later track, jump to the start.
fn prev_album_jump(queue: &[QueueEntry], current_pos: usize) -> Option<AlbumJump> {
    let (start, _) = album_run(queue, current_pos)?;
    if current_pos > start {
        let entry = &queue[start];
        return Some(AlbumJump {
            label: "Start of Album".to_string(),
            song_id: entry.id,
        });
    }

    if start == 0 {
        return None;
    }
    let (prev_start, _) = album_run(queue, start - 1)?;
    let entry = &queue[prev_start];
    Some(AlbumJump {
        label: format!("Prev Album: {}", album_display_name(entry)),
        song_id: entry.id,
    })
}

const EMPTY_QUEUE_ICON: &str = "♫";
const DISCONNECTED_ICON: &str = "🔇";
// Pinned to text presentation for the same reason as `PAUSED_ICON` below.
const ERROR_ICON: &str = "\u{26A0}\u{FE0E}";
// Pinned to text presentation with U+FE0E, as the menu glyphs below
// are, so it renders monochrome and narrow rather than as wide colour emoji.
const PAUSED_ICON: &str = "\u{23F8}\u{FE0E}";
// The compact title's other two states, pinned the same way.
const PLAYING_ICON: &str = "\u{25B6}\u{FE0E}";
const STOPPED_ICON: &str = "\u{23F9}\u{FE0E}";
const TITLE_MAX_CHARS: usize = 25;
const ELLIPSIS: char = '…';

/// Formats a song as "Title - Artist". Falls back to the song's
/// URL when there's no title tag, and omits the artist half when there are no
/// artist tags.
pub fn format_track(title: Option<&str>, url: &str, artists: &[String]) -> String {
    let title = title.unwrap_or(url);
    if artists.is_empty() {
        title.to_string()
    } else {
        format!("{title} - {}", artists.join(", "))
    }
}

/// Truncates `text` to at most `max_chars` characters, replacing the tail
/// with an ellipsis when truncation is needed. Operates on `char`s, not
/// bytes, so it never splits a multi-byte character.
pub fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut truncated: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    truncated.push(ELLIPSIS);
    truncated
}

/// The year to show for an album: `original_date`'s year, falling back to
/// `date`'s year, `None` if neither is present or usable. `album.rs`'s
/// worker calls this to populate `AlbumInfo::year` from a database row.
///
/// Extraction is just the first four bytes of the string — `original_date`
/// and `date` are always `YYYY`, `YYYY-MM` or `YYYY-MM-DD` — but guarded: only
/// accepted if all four bytes are ASCII digits, so a malformed value degrades
/// to "no year" rather than printing garbage into the menu bar.
pub fn year_of<'a>(original_date: Option<&'a str>, date: Option<&'a str>) -> Option<&'a str> {
    fn year_prefix(s: &str) -> Option<&str> {
        let prefix = s.get(0..4)?;
        prefix.bytes().all(|b| b.is_ascii_digit()).then_some(prefix)
    }
    original_date
        .and_then(year_prefix)
        .or_else(|| date.and_then(year_prefix))
}

pub fn track_title(state: PlayState, title: Option<&str>, url: &str, artists: &[String]) -> String {
    let truncated = truncate(&format_track(title, url, artists), TITLE_MAX_CHARS);
    if state == PlayState::Paused {
        format!("{PAUSED_ICON} {truncated}")
    } else {
        truncated
    }
}

/// The default menu-bar title: just a play-state glyph. Narrow on purpose —
/// macOS silently hides status items that don't fit beside a MacBook's
/// notch, and a track-length title is exactly the kind that doesn't. The
/// track itself is the first row of the dropdown instead.
pub fn compact_title(state: PlayState) -> String {
    match state {
        PlayState::Playing => PLAYING_ICON,
        PlayState::Paused => PAUSED_ICON,
        PlayState::Stopped => STOPPED_ICON,
    }
    .to_string()
}

pub fn empty_queue_title() -> String {
    EMPTY_QUEUE_ICON.to_string()
}

pub fn disconnected_title() -> String {
    DISCONNECTED_ICON.to_string()
}

pub fn error_title() -> String {
    ERROR_ICON.to_string()
}

/// `full_title` is `config.toml`'s opt-in for `Title - Artist`; otherwise
/// `compact_title`. Empty-queue, disconnected and error titles are single
/// glyphs either way.
pub fn title_for_snapshot(snapshot: &PlayerSnapshot, full_title: bool) -> String {
    match &snapshot.song {
        Some(_) if !full_title => compact_title(snapshot.status.state),
        Some(song) => track_title(
            snapshot.status.state,
            song.title(),
            &song.url,
            song.artists(),
        ),
        None => empty_queue_title(),
    }
}

/// "In playlists" contents: **all** playlists containing the
/// current song
fn in_playlists_list(playlists_for_url: &[String]) -> Vec<String> {
    let mut names = playlists_for_url.to_vec();
    names.sort_by_key(|name| name.to_lowercase());
    names
}

/// "Add to" contents: `key_playlists` minus whichever already
/// contain the song, preserving config order.
fn add_to_list(playlists_for_url: &[String], key_playlists: &[String]) -> Vec<String> {
    key_playlists
        .iter()
        .filter(|name| !playlists_for_url.contains(name))
        .cloned()
        .collect()
}

/// Everything outside RFC 3986's *unreserved* set gets percent-encoded. This
/// is deliberately more aggressive than the URL parser needs: it matches
/// Python's `quote(s, safe="")`.
/// It also has to survive a round trip through `Url::parse` on the way back
/// in — the payload lands in the URL's *opaque host* component (see
/// `main::decode_command_from_url`), where `/`, `:`, `?`, `#` and space are
/// all forbidden raw.
const URI_PAYLOAD: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// Wraps a library path as a single `shlex` token. The receiving end
/// re-tokenizes with `shlex::split` (`command::tokenize`), which honours
/// backslash escapes inside double quotes — so a path containing `"` or `\`
/// survives instead of producing an unbalanced-quote error.
fn quote_path(path: &str) -> String {
    let escaped = path.replace('\\', r"\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// The `mpdcontrol://` URI that queues `path` — a track file for the track
/// row, its parent directory for the album row (`add` is recursive, so a
/// directory adds the whole album). Copied to the clipboard for pasting into
/// Obsidian and friends; nothing in this app consumes it directly.
pub fn add_uri(path: &str) -> String {
    let command = format!("add {}", quote_path(path));
    format!(
        "{URL_SCHEME}://{}",
        utf8_percent_encode(&command, URI_PAYLOAD)
    )
}

/// A menu row that copies `uri` to the clipboard when clicked. Both the
/// current track and its album get one; the `uri` doubles as the menu item's
/// id, so dispatch needs no other state to act on the click.
#[derive(Debug, PartialEq, Eq)]
pub struct CopyRow {
    pub label: String,
    pub uri: String,
}

/// The current track's copy row, labelled with its "Title - Artist" text —
/// only cut at the dropdown's limit, so this row is also what shows the part
/// the tray title had to cut.
fn track_copy_row(current: &CurrentTrack<'_>) -> CopyRow {
    CopyRow {
        label: format_track(current.title, current.url, current.artists),
        uri: add_uri(current.url),
    }
}

/// The current album's copy row. `None` for a library-root file, which has no
/// parent directory and therefore no album to add.
fn album_copy_row(queue: &[QueueEntry], current: &CurrentTrack<'_>) -> Option<CopyRow> {
    let directory = album_key(current.url)?;
    Some(CopyRow {
        label: album_display_name(queue.get(current.pos)?),
        uri: add_uri(directory),
    })
}

/// Both album-nav targets, or `(None, None)` when the rows should be hidden:
/// nothing playing, empty queue, position out of bounds, `random` on, or the
/// run shorter than the threshold.
fn album_nav(
    state: PlayState,
    random: bool,
    queue: &[QueueEntry],
    current_pos: Option<usize>,
) -> (Option<AlbumJump>, Option<AlbumJump>) {
    if !matches!(state, PlayState::Playing | PlayState::Paused) {
        return (None, None);
    }
    let Some(pos) = current_pos else {
        return (None, None);
    };
    let Some((start, end)) = album_run(queue, pos) else {
        return (None, None);
    };
    if !show_album_nav(random, end - start + 1) {
        return (None, None);
    }
    (prev_album_jump(queue, pos), next_album_jump(queue, pos))
}

// No variation selector needed on any of these — U+2605, U+2606 and U+21BB
// have no emoji presentation form, unlike the icons above.
const STAR_FILLED: char = '\u{2605}'; // ★
const STAR_EMPTY: char = '\u{2606}'; // ☆
const RELISTEN_ICON: char = '\u{21BB}'; // ↻
const SUMMARY_SEPARATOR: &str = " \u{00B7} "; // " · " (U+00B7 MIDDLE DOT)

/// Five star glyphs for a `0`–`5` rating: `rating` filled stars followed by
/// `5 - rating` empty ones. A stored value outside `0`–`5` can't happen — the
/// schema's `CHECK` constraint prevents it — but must not panic here; it
/// simply renders as all-filled rather than indexing out of bounds.
pub fn stars(rating: u8) -> String {
    (0..5)
        .map(|i| if i < rating { STAR_FILLED } else { STAR_EMPTY })
        .collect()
}

/// Whether the `Rating ▸` submenu's `rating` item should render checked.
/// Trivial, but pulled out of `render` anyway — per this module's own rule,
/// decisions belong where they're testable, not inline in the
/// main-thread-only render walk.
pub fn rating_checked(info: &AlbumInfo, rating: u8) -> bool {
    info.rating == Some(rating)
}

/// Everything the album-database part of the menu needs: the disabled
/// summary/notice rows and the `Album ▸` submenu. Built from an `AlbumState`
/// by `album_section`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumSection {
    /// The summary row text; `None` means emit no row.
    pub summary: Option<String>,
    /// A disabled notice row (unavailable / write failed).
    pub notice: Option<String>,
    /// The row the `Album ▸` submenu's checkmarks mirror; `None` means emit
    /// no submenu at all.
    pub controls: Option<AlbumInfo>,
}

impl AlbumSection {
    /// No summary, no notice, no submenu — the album database has nothing to
    /// say about what's playing, or hasn't been asked yet.
    pub fn empty() -> Self {
        AlbumSection {
            summary: None,
            notice: None,
            controls: None,
        }
    }

    /// Whether every field is absent, i.e. this section contributes nothing
    /// to the menu. A `notice`-only section (e.g. "album database
    /// unavailable") is *not* empty — it still needs its row.
    pub fn is_empty(&self) -> bool {
        self.summary.is_none() && self.notice.is_none() && self.controls.is_none()
    }
}

/// The disabled summary row's text: year, star rating, relisten marker and
/// flag words, joined with `" · "` in that fixed order. `None` when every
/// part is absent — an unrated, undated, unflagged album gets no summary row
/// at all, not an empty one. See `docs/usage.md` for the rendered examples.
pub fn album_summary(info: &AlbumInfo) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(year) = &info.year {
        parts.push(year.clone());
    }
    if let Some(rating) = info.rating {
        parts.push(stars(rating));
    }
    if info.relisten {
        parts.push(RELISTEN_ICON.to_string());
    }
    if info.classical {
        parts.push("Classical".to_string());
    }
    if info.spoken {
        parts.push("Spoken".to_string());
    }
    if info.soundtrack {
        parts.push("Soundtrack".to_string());
    }
    (!parts.is_empty()).then(|| parts.join(SUMMARY_SEPARATOR))
}

const UNAVAILABLE_NOTICE: &str = "album database unavailable";
const WRITE_FAILED_NOTICE: &str = "album database write failed";
const NOT_FOUND_SUMMARY: &str = "not in album database";

/// Maps the worker's `AlbumState` (plus any pending write error) onto what
/// the menu shows. `write_error` takes priority over the state-derived
/// notice — it persists until the next successful write, covering the case
/// where the row is still readable but the last click didn't take.
pub fn album_section(state: &AlbumState, write_error: Option<&str>) -> AlbumSection {
    let notice = if write_error.is_some() {
        Some(WRITE_FAILED_NOTICE.to_string())
    } else if matches!(state, AlbumState::Unavailable) {
        Some(UNAVAILABLE_NOTICE.to_string())
    } else {
        None
    };

    let (summary, controls) = match state {
        AlbumState::NoAlbum | AlbumState::Unavailable => (None, None),
        AlbumState::NotFound => (Some(NOT_FOUND_SUMMARY.to_string()), None),
        AlbumState::Found(info) => (album_summary(info), Some(info.clone())),
    };

    AlbumSection {
        summary,
        notice,
        controls,
    }
}

/// Everything needed to render the dropdown menu, computed from primitives so
/// it's unit-testable without a live MPD connection. Carries no
/// tray title — that's set independently via `Display`.
pub struct MenuModel {
    pub track: Option<CopyRow>,
    /// The `⏭ Next` row's complete text, naming the next track when MPD
    /// reports one.
    pub next_label: String,
    pub album_title: Option<CopyRow>,
    pub prev_album: Option<AlbumJump>,
    pub next_album: Option<AlbumJump>,
    pub in_playlists: Vec<String>,
    pub add_to: Vec<String>,
    pub album_section: AlbumSection,
}

impl MenuModel {
    /// The model with nothing to show: no track context, no album nav, no
    /// playlists. Used at startup before the first MPD event and whenever the
    /// connection drops, leaving just the always-present action rows and Quit.
    pub fn empty() -> Self {
        MenuModel {
            track: None,
            next_label: next_label(None),
            album_title: None,
            prev_album: None,
            next_album: None,
            in_playlists: Vec::new(),
            add_to: Vec::new(),
            album_section: AlbumSection::empty(),
        }
    }

    /// Whether the album-*navigation* section (and the separator preceding
    /// it) should be emitted at all — the prev/next rows, not `album_section`,
    /// which is the album *database* row. Distinct from the free
    /// `show_album_nav` above, which decides whether the jumps are worth
    /// computing in the first place; this one only asks whether any survived.
    /// Lives here rather than in `render`, which can't be tested (see the
    /// module note on `muda::Menu` being main-thread-only). Derived rather
    /// than stored, so it can't drift from the fields it's computed from.
    pub fn show_album_nav_section(&self) -> bool {
        self.prev_album.is_some() || self.next_album.is_some()
    }

    /// Whether the current-track section (the album-title copy row, "In
    /// playlists", "Add to", and the album-database section) and its
    /// preceding separator should be emitted. The track's own copy row isn't
    /// part of it: that sits above the controls, as the menu's header. Same rule and same
    /// reasoning as `show_album_nav_section`. A `notice`-only row (e.g. "album
    /// database unavailable") still needs the section to show even with
    /// nothing else in it, so `AlbumSection::is_empty` covers all three of
    /// its fields, not just `controls`.
    pub fn show_track_section(&self) -> bool {
        self.album_title.is_some()
            || !self.in_playlists.is_empty()
            || !self.add_to.is_empty()
            || !self.album_section.is_empty()
    }
}

/// The playing track, as far as the menu cares: where it sits in the queue,
/// and how to name it. Bundled rather than passed as four loose parameters so
/// the type enforces "all present or all absent" — the earlier signature let
/// `current_pos` be `Some` while the track itself was `None`, a combination
/// with no meaning that every consumer had to defend against anyway.
pub struct CurrentTrack<'a> {
    pub pos: usize,
    /// MPD's `nextsong` position — already accounts for `random`, `repeat`
    /// and `single`, so nothing here re-derives play order from the queue.
    /// Lives here because MPD only reports it alongside a current song.
    pub next_pos: Option<usize>,
    pub url: &'a str,
    pub title: Option<&'a str>,
    pub artists: &'a [String],
}

/// The next track as `Title - Artist`, but with the artist dropped when it's the same as the current track's, which
/// on an album is almost always, and would just repeat what the menu's first
/// row already says.
fn next_track_text(queue: &[QueueEntry], current: &CurrentTrack<'_>) -> Option<String> {
    let next = queue.get(current.next_pos?)?;
    let artists: &[String] = if next.artists == current.artists {
        &[]
    } else {
        &next.artists
    };
    Some(format_track(next.title.as_deref(), &next.url, artists))
}

fn next_label(next_track: Option<String>) -> String {
    match next_track {
        Some(text) => format!("Next: {text}"),
        None => "Next".to_string(),
    }
}

/// Pure: assembles a `MenuModel` from already-extracted primitives. All the
/// album-nav and playlist rules live here; `render` makes no further decisions.
///
/// `max_chars` (`config.toml`'s `menu_text_max_chars`) is applied to each
/// finished label, not to its parts, so every row — track, next, album,
/// album-nav — is cut at the same visible width. Cutting the tail never
/// reaches a fixed prefix like `Next Album: ` at any sensible limit.
pub fn model(
    state: PlayState,
    random: bool,
    queue: &[QueueEntry],
    current: Option<CurrentTrack<'_>>,
    playlists_for_url: &[String],
    key_playlists: &[String],
    max_chars: usize,
) -> MenuModel {
    let cut = |label: String| truncate(&label, max_chars);
    let cut_row = |row: CopyRow| CopyRow {
        label: cut(row.label),
        ..row
    };
    let cut_jump = |jump: AlbumJump| AlbumJump {
        label: cut(jump.label),
        ..jump
    };

    let track = current.as_ref().map(track_copy_row).map(cut_row);
    let next_label = cut(next_label(
        current
            .as_ref()
            .and_then(|current| next_track_text(queue, current)),
    ));
    let album_title = current
        .as_ref()
        .and_then(|current| album_copy_row(queue, current))
        .map(cut_row);
    let (prev_album, next_album) = album_nav(
        state,
        random,
        queue,
        current.as_ref().map(|track| track.pos),
    );

    MenuModel {
        track,
        next_label,
        album_title,
        prev_album: prev_album.map(cut_jump),
        next_album: next_album.map(cut_jump),
        in_playlists: in_playlists_list(playlists_for_url),
        add_to: add_to_list(playlists_for_url, key_playlists),
        // The caller (`main.rs`) carries the previous model's `album_section`
        // forward across an `MpdEvent` — this constructor has no access to
        // the album-db worker's state, so it always starts empty.
        album_section: AlbumSection::empty(),
    }
}

// All glyphs are pinned to text presentation with U+FE0E so they
// render monochrome and narrow, matching the menu-bar font weight, rather
// than falling back to wide colour emoji.
const PLAY_PAUSE_ICON: &str = "\u{23EF}\u{FE0E}";
const NEXT_ICON: &str = "\u{23ED}\u{FE0E}";
const CLEAR_ICON: &str = "\u{23F9}\u{FE0E}";
const PREV_ALBUM_ICON: &str = "\u{23EE}\u{FE0E}\u{23EE}\u{FE0E}";
const NEXT_ALBUM_ICON: &str = "\u{23ED}\u{FE0E}\u{23ED}\u{FE0E}";
// U+29C9 "two joined squares" — the copy/duplicate glyph macOS uses in text
// contexts. Deliberately not 📋 (U+1F4CB): that codepoint has no text
// presentation form in the system fonts, so `U+FE0E` can't tame it and it
// renders as wide colour emoji against the menu's monochrome glyphs.
const COPY_ICON: &str = "\u{29C9}";

// The three dynamic menu-id families. Both the builders and the
// `strip_*` parsers live here so `main.rs` dispatch never has to know the wire
// format — otherwise the halves sit in different modules with nothing keeping
// them in step.
const ALBUM_ID_PREFIX: &str = "album:";
const ADDTO_ID_PREFIX: &str = "addto:";
const COPY_ID_PREFIX: &str = "copy:";
// ⚠ Must never start with `album` — `strip_album_id` uses `strip_prefix`, so
// today's `album:` navigation prefix and this one are unambiguous, but a
// careless future rename could collide.
const RATING_ID_PREFIX: &str = "rating:";
const FLAG_ID_PREFIX: &str = "flag:";

fn album_jump_item_id(song_id: u64) -> String {
    format!("{ALBUM_ID_PREFIX}{song_id}")
}

fn addto_item_id(name: &str) -> String {
    format!("{ADDTO_ID_PREFIX}{name}")
}

/// The copy rows carry their whole payload in the id, so a click needs no
/// lookup against state that may have moved on since the menu was built —
/// unlike "Add to", which acts on the song the menu was built from and so has
/// to reach for `main.rs`'s remembered URL.
fn copy_item_id(uri: &str) -> String {
    format!("{COPY_ID_PREFIX}{uri}")
}

fn rating_item_id(rating: u8) -> String {
    format!("{RATING_ID_PREFIX}{rating}")
}

/// `flag`'s wire-format name (`relisten`/`classical`/`spoken`/`soundtrack`) —
/// distinct from `AlbumFlag::column()`'s SQL column names, which are an
/// internal `albumdb` detail this module has no reason to leak into the
/// menu's ids. The inverse of `parse_flag_name`.
fn flag_name(flag: AlbumFlag) -> &'static str {
    match flag {
        AlbumFlag::Relisten => "relisten",
        AlbumFlag::Classical => "classical",
        AlbumFlag::Spoken => "spoken",
        AlbumFlag::Soundtrack => "soundtrack",
    }
}

/// The inverse of `flag_name`, or `None` for anything else.
fn parse_flag_name(name: &str) -> Option<AlbumFlag> {
    match name {
        "relisten" => Some(AlbumFlag::Relisten),
        "classical" => Some(AlbumFlag::Classical),
        "spoken" => Some(AlbumFlag::Spoken),
        "soundtrack" => Some(AlbumFlag::Soundtrack),
        _ => None,
    }
}

/// `value` is the value a click on this item should *write* — the target
/// state, not a toggle instruction. That matters if the row changed
/// underneath between the menu being built and the item being clicked: you
/// get the value you actually clicked for, not an inversion of some state
/// you never saw.
fn flag_item_id(flag: AlbumFlag, value: bool) -> String {
    format!("{FLAG_ID_PREFIX}{}:{}", flag_name(flag), u8::from(value))
}

/// The song-id text of an album-nav menu id, or `None` if `id` isn't one.
/// Parsing the number is left to the caller, so it can log a malformed id
/// distinctly from an id belonging to some other item.
pub fn strip_album_id(id: &str) -> Option<&str> {
    id.strip_prefix(ALBUM_ID_PREFIX)
}

/// The playlist name in an "Add to" menu id, or `None` if `id` isn't one.
/// Names may contain `:`, so the whole remainder after the fixed prefix is the
/// name — this never splits on an interior colon.
pub fn strip_addto_id(id: &str) -> Option<&str> {
    id.strip_prefix(ADDTO_ID_PREFIX)
}

/// The `mpdcontrol://` URI to copy, or `None` if `id` isn't a copy row's.
pub fn strip_copy_id(id: &str) -> Option<&str> {
    id.strip_prefix(COPY_ID_PREFIX)
}

/// The rating text (`"0"`–`"5"`) of a `rating:` menu id, or `None` if `id`
/// isn't one. Parsing to `u8` is left to the caller, so it can log a
/// malformed id distinctly from an id belonging to some other item — same
/// pattern as `strip_album_id`.
pub fn strip_rating_id(id: &str) -> Option<&str> {
    id.strip_prefix(RATING_ID_PREFIX)
}

/// The `"<name>:<0|1>"` remainder of a `flag:` menu id, or `None` if `id`
/// isn't one. Splitting the flag name from the target value is left to the
/// caller, same pattern as `strip_rating_id` above.
pub fn strip_flag_id(id: &str) -> Option<&str> {
    id.strip_prefix(FLAG_ID_PREFIX)
}

/// Parses `strip_flag_id`'s output the rest of the way: the flag it names and
/// the value a click should write. `None` for anything malformed — an
/// unknown name, a missing `:`, or a value that isn't exactly `0`/`1`. Kept
/// here rather than in `main.rs` so the flag-name spelling has one home, not
/// two: `main.rs` dispatches on the parsed `AlbumFlag`, never the wire string.
pub fn parse_flag_payload(payload: &str) -> Option<(AlbumFlag, bool)> {
    let (name, value) = payload.split_once(':')?;
    let flag = parse_flag_name(name)?;
    let value = match value {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    Some((flag, value))
}

/// Turns an already-computed `MenuModel` into `muda` items. Every decision
/// lives in `model()` and `MenuModel`'s `show_*_section` methods, which is
/// what keeps them testable: **this function cannot be unit-tested**, because
/// `muda::Menu::new` panics off the main thread and `cargo test` runs tests on
/// spawned threads. Keep decisions out of here.
pub fn render(model: &MenuModel) -> Result<Menu> {
    let menu = Menu::new();

    // First, so the menu still says what's playing when the tray title is
    // just `compact_title`'s glyph.
    if let Some(row) = &model.track {
        menu.append(&copy_item(row))?;
        menu.append(&PredefinedMenuItem::separator())?;
    }

    menu.append(&MenuItem::with_id(
        PLAY_PAUSE_ID,
        format!("{PLAY_PAUSE_ICON} Play/Pause"),
        true,
        None,
    ))?;
    menu.append(&MenuItem::with_id(
        NEXT_ID,
        format!("{NEXT_ICON} {}", model.next_label),
        true,
        None,
    ))?;
    menu.append(&MenuItem::with_id(
        CLEAR_ID,
        format!("{CLEAR_ICON} Clear"),
        true,
        None,
    ))?;

    if model.show_album_nav_section() {
        menu.append(&PredefinedMenuItem::separator())?;
        if let Some(jump) = &model.prev_album {
            menu.append(&MenuItem::with_id(
                album_jump_item_id(jump.song_id),
                format!("{PREV_ALBUM_ICON} {}", jump.label),
                true,
                None,
            ))?;
        }
        if let Some(jump) = &model.next_album {
            menu.append(&MenuItem::with_id(
                album_jump_item_id(jump.song_id),
                format!("{NEXT_ALBUM_ICON} {}", jump.label),
                true,
                None,
            ))?;
        }
    }

    if model.show_track_section() {
        menu.append(&PredefinedMenuItem::separator())?;
        if let Some(row) = &model.album_title {
            menu.append(&copy_item(row))?;
        }
        if let Some(summary) = &model.album_section.summary {
            menu.append(&MenuItem::new(summary, false, None))?;
        }
        if let Some(notice) = &model.album_section.notice {
            menu.append(&MenuItem::new(notice, false, None))?;
        }
        if let Some(controls) = &model.album_section.controls {
            let album_submenu = Submenu::new("Album", true);

            let rating_submenu = Submenu::new("Rating", true);
            for rating in 0..=5u8 {
                rating_submenu.append(&CheckMenuItem::with_id(
                    rating_item_id(rating),
                    format!("{} {rating}", stars(rating)),
                    true,
                    rating_checked(controls, rating),
                    None,
                ))?;
            }
            album_submenu.append(&rating_submenu)?;
            album_submenu.append(&PredefinedMenuItem::separator())?;

            for (flag, label, checked) in [
                (AlbumFlag::Relisten, "Relisten", controls.relisten),
                (AlbumFlag::Classical, "Classical", controls.classical),
                (AlbumFlag::Spoken, "Spoken", controls.spoken),
                (AlbumFlag::Soundtrack, "Soundtrack", controls.soundtrack),
            ] {
                album_submenu.append(&CheckMenuItem::with_id(
                    flag_item_id(flag, !checked),
                    label,
                    true,
                    checked,
                    None,
                ))?;
            }
            menu.append(&album_submenu)?;
        }
        if !model.in_playlists.is_empty() {
            let submenu = Submenu::new("In playlists", true);
            for name in &model.in_playlists {
                submenu.append(&MenuItem::new(name, false, None))?;
            }
            menu.append(&submenu)?;
        }
        if !model.add_to.is_empty() {
            let submenu = Submenu::new("Add to", true);
            for name in &model.add_to {
                submenu.append(&MenuItem::with_id(addto_item_id(name), name, true, None))?;
            }
            menu.append(&submenu)?;
        }
    }

    // Block 1 (Play/Pause/Next/Clear) is always present, so this separator
    // and Quit are always appended — never leaving two separators adjacent.
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&MenuItem::with_id(QUIT_ID, "Quit", true, None))?;

    Ok(menu)
}

fn copy_item(row: &CopyRow) -> MenuItem {
    MenuItem::with_id(
        copy_item_id(&row.uri),
        format!("{COPY_ICON} {}", row.label),
        true,
        None,
    )
}

/// The startup-error menu: just what broke and where to look, plus Quit.
/// Used in place of `render` when `config::load` fails — nothing else in the
/// app (MPD connection included) is even attempted in that case, so there is
/// nothing else to show. The description rows are disabled `MenuItem`s
/// (`false` for `enabled`), the same convention `render` uses for the
/// informational "In playlists" rows.
pub fn render_error(message: &str, log_path: Option<&Path>) -> Result<Menu> {
    let menu = Menu::new();

    menu.append(&MenuItem::new(message, false, None))?;
    if let Some(log_path) = log_path {
        menu.append(&MenuItem::new(
            format!("Log: {}", log_path.display()),
            false,
            None,
        ))?;
    }
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&MenuItem::with_id(QUIT_ID, "Quit", true, None))?;

    Ok(menu)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn format_track_combines_title_and_artists() {
        let one = vec!["My Artist".to_string()];
        let two = vec!["Artist A".to_string(), "Artist B".to_string()];
        assert_eq!(
            format_track(Some("My Title"), "foo.mp3", &one),
            "My Title - My Artist"
        );
        assert_eq!(
            format_track(Some("My Title"), "foo.mp3", &two),
            "My Title - Artist A, Artist B"
        );
        // No artist tags: the whole " - Artist" half is dropped, not left dangling.
        assert_eq!(format_track(Some("My Title"), "foo.mp3", &[]), "My Title");
        // No title tag: the url stands in for it.
        assert_eq!(
            format_track(None, "Album/track.mp3", &one),
            "Album/track.mp3 - My Artist"
        );
    }

    #[test]
    fn truncate_leaves_text_exactly_at_limit_untouched() {
        let text = "a".repeat(25);
        assert_eq!(truncate(&text, 25), text);
    }

    #[test]
    fn truncate_shortens_text_just_over_limit() {
        let text = "a".repeat(26);
        let result = truncate(&text, 25);
        assert_eq!(result.chars().count(), 25);
        assert_eq!(result, format!("{}{}", "a".repeat(24), ELLIPSIS));
    }

    #[test]
    fn year_of_prefers_original_date_and_rejects_unusable_values() {
        // Every format the two columns are stored in.
        assert_eq!(year_of(Some("1996"), None), Some("1996"));
        assert_eq!(year_of(Some("1996-05"), None), Some("1996"));
        assert_eq!(year_of(Some("1996-05-12"), None), Some("1996"));

        assert_eq!(year_of(None, Some("2003-01-01")), Some("2003"));
        assert_eq!(year_of(None, None), None);

        // A non-numeric leading four characters falls through to `date`, and
        // then to `None` — never into the menu.
        assert_eq!(year_of(Some("abcd-05"), Some("2003")), Some("2003"));
        assert_eq!(year_of(Some("abcd"), None), None);
    }

    #[test]
    fn truncate_is_unicode_safe() {
        // Multi-byte characters throughout; must not panic or split a char.
        let text = "🎵".repeat(30);
        let result = truncate(&text, 25);
        assert_eq!(result.chars().count(), 25);
        assert!(result.ends_with(ELLIPSIS));
    }

    #[test]
    fn track_title_omits_paused_glyph_when_playing_or_stopped() {
        let playing = track_title(PlayState::Playing, Some("My Title"), "foo.mp3", &[]);
        assert_eq!(playing, "My Title");
        let stopped = track_title(PlayState::Stopped, Some("My Title"), "foo.mp3", &[]);
        assert_eq!(stopped, "My Title");
    }

    #[test]
    fn track_title_paused_prefix_sits_outside_the_truncation_budget() {
        // The glyph is prepended *after* truncation, so text is cut at exactly
        // the same point whether paused or playing. Checked at the budget
        // boundary and well past it.
        for title in [&"a".repeat(TITLE_MAX_CHARS), &"a".repeat(40)] {
            let playing = track_title(PlayState::Playing, Some(title), "foo.mp3", &[]);
            let paused = track_title(PlayState::Paused, Some(title), "foo.mp3", &[]);
            assert_eq!(paused, format!("{PAUSED_ICON} {playing}"));
        }
    }

    #[test]
    fn compact_title_is_one_glyph_per_play_state() {
        assert_eq!(compact_title(PlayState::Playing), PLAYING_ICON);
        assert_eq!(compact_title(PlayState::Paused), PAUSED_ICON);
        assert_eq!(compact_title(PlayState::Stopped), STOPPED_ICON);
    }

    fn titled_entry(title: &str, artist: &str) -> QueueEntry {
        QueueEntry {
            title: Some(title.to_string()),
            artists: vec![artist.to_string()],
            ..entry("A/Al/x.mp3")
        }
    }

    fn current_at(pos: usize, next_pos: Option<usize>, artists: &[String]) -> CurrentTrack<'_> {
        CurrentTrack {
            pos,
            next_pos,
            url: "A/Al/x.mp3",
            title: Some("Current"),
            artists,
        }
    }

    #[test]
    fn next_track_text_drops_the_artist_when_unchanged() {
        let queue = vec![
            titled_entry("Top of the Hill", "Tom Waits"),
            titled_entry("Hoist That Rag", "Tom Waits"),
        ];
        let artists = vec!["Tom Waits".to_string()];
        assert_eq!(
            next_track_text(&queue, &current_at(0, Some(1), &artists)).as_deref(),
            Some("Hoist That Rag")
        );
    }

    #[test]
    fn next_track_text_shows_a_different_artist() {
        let queue = vec![
            titled_entry("Top of the Hill", "Tom Waits"),
            titled_entry("Sins of My Father", "Someone Else Entirely"),
        ];
        let artists = vec!["Tom Waits".to_string()];
        assert_eq!(
            next_track_text(&queue, &current_at(0, Some(1), &artists)).as_deref(),
            Some("Sins of My Father - Someone Else Entirely")
        );
    }

    #[test]
    fn model_cuts_every_dropdown_label_at_the_same_limit() {
        let long = "x".repeat(100);
        let queue: Vec<QueueEntry> = (0..10u64)
            .map(|i| QueueEntry {
                title: Some(long.clone()),
                ..queue_entry(&format!("A/Al{}/{i}.mp3", i / 5), Some(&long), i)
            })
            .collect();
        let model = model(
            PlayState::Playing,
            false,
            &queue,
            Some(CurrentTrack {
                pos: 1,
                next_pos: Some(2),
                url: &queue[1].url,
                title: Some(&long),
                artists: &[],
            }),
            &[],
            &[],
            30,
        );
        let labels = [
            model.track.unwrap().label,
            model.next_label,
            model.album_title.unwrap().label,
            model.next_album.unwrap().label,
        ];
        for label in labels {
            assert_eq!(label.chars().count(), 30, "{label}");
            assert!(label.ends_with(ELLIPSIS), "{label}");
        }
    }

    #[test]
    fn next_label_is_plain_without_a_next_track() {
        let queue = vec![titled_entry("Only", "Tom Waits")];
        let artists = vec!["Tom Waits".to_string()];
        // No `nextsong` (end of queue), or one that doesn't resolve.
        assert_eq!(
            next_track_text(&queue, &current_at(0, None, &artists)),
            None
        );
        assert_eq!(
            next_track_text(&queue, &current_at(0, Some(5), &artists)),
            None
        );
        assert_eq!(next_label(None), "Next");
        assert_eq!(
            next_label(Some("Hoist That Rag".to_string())),
            "Next: Hoist That Rag"
        );
    }

    fn entry(url: &str) -> QueueEntry {
        QueueEntry {
            url: url.to_string(),
            album: None,
            title: None,
            artists: Vec::new(),
            id: 0,
        }
    }

    #[test]
    fn album_key_groups_by_directory() {
        // Every multi-disc album in this library lives in one directory with
        // continuous track numbering, so directory grouping
        // handles them with no CD1/CD2 special-casing.
        assert_eq!(
            album_key("Beatles/Abbey Road/01 Come Together.mp3"),
            album_key("Beatles/Abbey Road/02 Something.mp3")
        );
        assert_ne!(
            album_key("Beatles/Abbey Road/01 Come Together.mp3"),
            album_key("Beatles/Let It Be/01 Two of Us.mp3")
        );
    }

    #[test]
    fn album_key_root_level_file_has_no_parent() {
        assert_eq!(album_key("loose_track.mp3"), None);
    }

    #[test]
    fn album_run_at_start_middle_end_of_run() {
        let queue = vec![
            entry("A/Al/1.mp3"),
            entry("A/Al/2.mp3"),
            entry("A/Al/3.mp3"),
        ];
        assert_eq!(album_run(&queue, 0), Some((0, 2)));
        assert_eq!(album_run(&queue, 1), Some((0, 2)));
        assert_eq!(album_run(&queue, 2), Some((0, 2)));
    }

    #[test]
    fn album_run_stops_at_album_boundary() {
        let queue = vec![
            entry("A/Al1/1.mp3"),
            entry("A/Al1/2.mp3"),
            entry("A/Al2/1.mp3"),
            entry("A/Al2/2.mp3"),
        ];
        assert_eq!(album_run(&queue, 1), Some((0, 1)));
        assert_eq!(album_run(&queue, 2), Some((2, 3)));
    }

    #[test]
    fn album_run_single_track_run_has_length_one() {
        let queue = vec![
            entry("A/Al1/1.mp3"),
            entry("A/Al2/1.mp3"),
            entry("A/Al3/1.mp3"),
        ];
        assert_eq!(album_run(&queue, 1), Some((1, 1)));
    }

    #[test]
    fn album_run_position_outside_the_queue_is_none() {
        // An empty queue is just the degenerate case of the same bounds check.
        let queue = vec![entry("A/Al/1.mp3"), entry("A/Al/2.mp3")];
        assert_eq!(album_run(&queue, 5), None);
        assert_eq!(album_run(&[], 0), None);
    }

    fn queue_entry(url: &str, album: Option<&str>, id: u64) -> QueueEntry {
        QueueEntry {
            url: url.to_string(),
            album: album.map(str::to_string),
            title: None,
            artists: Vec::new(),
            id,
        }
    }

    #[test]
    fn show_album_nav_needs_a_run_of_five_and_no_random() {
        assert!(show_album_nav(false, 5));
        assert!(!show_album_nav(false, 4));
        // Random beats any run length — queue-order jumps are meaningless.
        assert!(!show_album_nav(true, 100));
    }

    #[test]
    fn album_display_name_uses_album_tag() {
        let entry = queue_entry("Beatles/Abbey Road/1.mp3", Some("Abbey Road"), 1);
        assert_eq!(album_display_name(&entry), "Abbey Road");
    }

    #[test]
    fn album_display_name_falls_back_to_directory_basename() {
        let entry = queue_entry("Beatles/Abbey Road/1.mp3", None, 1);
        assert_eq!(album_display_name(&entry), "Abbey Road");
    }

    #[test]
    fn next_album_jump_returns_run_first_entry() {
        let queue = vec![
            queue_entry("A/Abbey Road/1.mp3", Some("Abbey Road"), 1),
            queue_entry("A/Abbey Road/2.mp3", Some("Abbey Road"), 2),
            queue_entry("A/Let It Be/1.mp3", Some("Let It Be"), 3),
            queue_entry("A/Let It Be/2.mp3", Some("Let It Be"), 4),
        ];
        let jump = next_album_jump(&queue, 0).unwrap();
        assert_eq!(jump.song_id, 3);
        assert_eq!(jump.label, "Next Album: Let It Be");
    }

    #[test]
    fn next_album_jump_at_last_album_is_none() {
        let queue = vec![
            queue_entry("A/Abbey Road/1.mp3", Some("Abbey Road"), 1),
            queue_entry("A/Abbey Road/2.mp3", Some("Abbey Road"), 2),
        ];
        assert_eq!(next_album_jump(&queue, 0), None);
    }

    #[test]
    fn prev_album_jump_on_track_one_returns_preceding_run_labelled() {
        let queue = vec![
            queue_entry("A/Abbey Road/1.mp3", Some("Abbey Road"), 1),
            queue_entry("A/Abbey Road/2.mp3", Some("Abbey Road"), 2),
            queue_entry("A/Let It Be/1.mp3", Some("Let It Be"), 3),
            queue_entry("A/Let It Be/2.mp3", Some("Let It Be"), 4),
        ];
        // Position 2 is track 1 of the "Let It Be" run.
        let jump = prev_album_jump(&queue, 2).unwrap();
        assert_eq!(jump.song_id, 1);
        assert_eq!(jump.label, "Prev Album: Abbey Road");
    }

    #[test]
    fn prev_album_jump_on_track_two_returns_start_of_album() {
        let queue = vec![
            queue_entry("A/Abbey Road/1.mp3", Some("Abbey Road"), 1),
            queue_entry("A/Abbey Road/2.mp3", Some("Abbey Road"), 2),
            queue_entry("A/Abbey Road/3.mp3", Some("Abbey Road"), 3),
        ];
        let jump = prev_album_jump(&queue, 1).unwrap();
        assert_eq!(jump.song_id, 1);
        assert_eq!(jump.label, "Start of Album");
    }

    #[test]
    fn prev_album_jump_two_clicks_reaches_previous_album() {
        let queue = vec![
            queue_entry("A/Please Please Me/1.mp3", Some("Please Please Me"), 1),
            queue_entry("A/Abbey Road/1.mp3", Some("Abbey Road"), 2),
            queue_entry("A/Abbey Road/2.mp3", Some("Abbey Road"), 3),
            queue_entry("A/Abbey Road/3.mp3", Some("Abbey Road"), 4),
            queue_entry("A/Abbey Road/4.mp3", Some("Abbey Road"), 5),
            queue_entry("A/Abbey Road/5.mp3", Some("Abbey Road"), 6),
        ];
        // First click from track 5 (pos 5) lands on track 1 (pos 1, id 2).
        let first = prev_album_jump(&queue, 5).unwrap();
        assert_eq!(first.song_id, 2);
        assert_eq!(first.label, "Start of Album");

        // Second click, now positioned at track 1, reaches the previous album.
        let second = prev_album_jump(&queue, 1).unwrap();
        assert_eq!(second.song_id, 1);
        assert_eq!(second.label, "Prev Album: Please Please Me");
    }

    #[test]
    fn prev_album_jump_at_track_one_of_first_album_is_none() {
        let queue = vec![
            queue_entry("A/Abbey Road/1.mp3", Some("Abbey Road"), 1),
            queue_entry("A/Abbey Road/2.mp3", Some("Abbey Road"), 2),
        ];
        assert_eq!(prev_album_jump(&queue, 0), None);
    }

    #[test]
    fn in_playlists_list_sorts_case_insensitively() {
        let playlists = vec![
            "zebra".to_string(),
            "Five stars".to_string(),
            "album".to_string(),
        ];
        assert_eq!(
            in_playlists_list(&playlists),
            vec![
                "album".to_string(),
                "Five stars".to_string(),
                "zebra".to_string()
            ]
        );
    }

    #[test]
    fn add_to_list_excludes_playlists_already_containing_song() {
        let playlists_for_url = vec!["Five stars".to_string()];
        let key_playlists = vec![
            "Five stars".to_string(),
            "My Top Rated".to_string(),
            "mellow_2024".to_string(),
        ];
        assert_eq!(
            add_to_list(&playlists_for_url, &key_playlists),
            vec!["My Top Rated".to_string(), "mellow_2024".to_string()]
        );
    }

    #[test]
    fn add_to_list_song_in_no_playlists_offers_full_key_playlists_in_config_order() {
        let key_playlists = vec![
            "zebra".to_string(),
            "Five stars".to_string(),
            "album".to_string(),
        ];
        // Config order, deliberately not the alphabetical order "In playlists"
        // uses.
        assert_eq!(add_to_list(&[], &key_playlists), key_playlists);
    }

    #[test]
    fn model_hides_album_nav_when_stopped_even_with_a_long_run() {
        let queue = vec![
            queue_entry("A/Al/1.mp3", Some("Al"), 1),
            queue_entry("A/Al/2.mp3", Some("Al"), 2),
            queue_entry("A/Al/3.mp3", Some("Al"), 3),
            queue_entry("A/Al/4.mp3", Some("Al"), 4),
            queue_entry("A/Al/5.mp3", Some("Al"), 5),
        ];
        let model = model(
            PlayState::Stopped,
            false,
            &queue,
            Some(CurrentTrack {
                pos: 2,
                next_pos: None,
                url: &queue[2].url,
                title: Some("Track 3"),
                artists: &[],
            }),
            &[],
            &[],
            60,
        );
        assert!(model.prev_album.is_none());
        assert!(model.next_album.is_none());
    }

    #[test]
    fn model_end_to_end_assembles_expected_menu_model() {
        let queue = vec![
            queue_entry("Beatles/Abbey Road/1.mp3", Some("Abbey Road"), 1),
            queue_entry("Beatles/Abbey Road/2.mp3", Some("Abbey Road"), 2),
            queue_entry("Beatles/Abbey Road/3.mp3", Some("Abbey Road"), 3),
            queue_entry("Beatles/Abbey Road/4.mp3", Some("Abbey Road"), 4),
            queue_entry("Beatles/Abbey Road/5.mp3", Some("Abbey Road"), 5),
            queue_entry("Beatles/Let It Be/1.mp3", Some("Let It Be"), 6),
        ];
        let playlists_for_url = vec!["Five stars".to_string()];
        let key_playlists = vec!["Five stars".to_string(), "My Top Rated".to_string()];
        // Long enough to force truncation, so this also exercises the
        // `full_text` wiring through to `track_display`, not just the
        // album-nav/playlist assembly.
        let artists = vec!["The Beatles (Remastered 2019 Edition)".to_string()];

        let model = model(
            PlayState::Playing,
            false,
            &queue,
            Some(CurrentTrack {
                pos: 2,
                next_pos: None,
                url: &queue[2].url,
                title: Some("Come Together"),
                artists: &artists,
            }),
            &playlists_for_url,
            &key_playlists,
            60,
        );

        let track = model.track.unwrap();
        assert_eq!(
            track.label,
            "Come Together - The Beatles (Remastered 2019 Edition)"
        );
        assert_eq!(
            track.uri,
            "mpdcontrol://add%20%22Beatles%2FAbbey%20Road%2F3.mp3%22"
        );
        let album = model.album_title.unwrap();
        assert_eq!(album.label, "Abbey Road");
        assert_eq!(album.uri, "mpdcontrol://add%20%22Beatles%2FAbbey%20Road%22");
        let prev = model.prev_album.unwrap();
        assert_eq!(prev.label, "Start of Album");
        assert_eq!(prev.song_id, 1);
        let next = model.next_album.unwrap();
        assert_eq!(next.label, "Next Album: Let It Be");
        assert_eq!(next.song_id, 6);
        assert_eq!(model.in_playlists, vec!["Five stars".to_string()]);
        assert_eq!(model.add_to, vec!["My Top Rated".to_string()]);
    }

    fn model_with(
        track: Option<&str>,
        prev_album: Option<AlbumJump>,
        in_playlists: Vec<String>,
        add_to: Vec<String>,
    ) -> MenuModel {
        MenuModel {
            track: track.map(|label| CopyRow {
                label: label.to_string(),
                uri: add_uri("Al/1.mp3"),
            }),
            next_label: next_label(None),
            album_title: None,
            prev_album,
            next_album: None,
            in_playlists,
            add_to,
            album_section: AlbumSection::empty(),
        }
    }

    fn a_jump() -> AlbumJump {
        AlbumJump {
            label: "Start of Album".to_string(),
            song_id: 1,
        }
    }

    #[test]
    fn sections_are_hidden_when_they_would_be_empty() {
        // Both false means `render` emits no separator for either section —
        // the "never two adjacent separators" rule.
        let model = model_with(None, None, vec![], vec![]);
        assert!(!model.show_album_nav_section());
        assert!(!model.show_track_section());
    }

    #[test]
    fn album_nav_shows_when_either_jump_is_present() {
        assert!(model_with(None, Some(a_jump()), vec![], vec![]).show_album_nav_section());

        let next_only = MenuModel {
            next_album: Some(a_jump()),
            ..model_with(None, None, vec![], vec![])
        };
        assert!(next_only.show_album_nav_section());
    }

    #[test]
    fn track_section_shows_when_any_of_its_parts_is_present() {
        // The track row alone doesn't count: it's rendered above the
        // controls, not in this section.
        assert!(!model_with(Some("Some Track"), None, vec![], vec![]).show_track_section());
        assert!(
            model_with(None, None, vec!["Five stars".to_string()], vec![]).show_track_section()
        );
        assert!(
            model_with(None, None, vec![], vec!["My Top Rated".to_string()]).show_track_section()
        );

        let album_only = MenuModel {
            album_title: Some(CopyRow {
                label: "Abbey Road".to_string(),
                uri: add_uri("Beatles/Abbey Road"),
            }),
            ..model_with(None, None, vec![], vec![])
        };
        assert!(album_only.show_track_section());
    }

    #[test]
    fn add_uri_matches_the_hand_written_album_and_track_examples() {
        // The two URIs that were pasted into Obsidian by hand before this
        // existed — byte-for-byte, so links from both sources stay identical.
        assert_eq!(
            add_uri("Albums/B/Billy Bragg/Tooth & Nail"),
            "mpdcontrol://add%20%22Albums%2FB%2FBilly%20Bragg%2FTooth%20%26%20Nail%22"
        );
        assert_eq!(
            add_uri("Albums/T/They Might Be Giants/They Might Be Giants/03 Number Three.mp3"),
            "mpdcontrol://add%20%22Albums%2FT%2FThey%20Might%20Be%20Giants%2FThey%20Might%20Be%20Giants%2F03%20Number%20Three.mp3%22"
        );
    }

    #[test]
    fn add_uri_leaves_the_unreserved_set_alone() {
        // Python's `quote(s, safe="")` encodes everything but these four
        // punctuation marks; `%` itself must be encoded, or decoding the URI
        // would eat the following two characters.
        assert_eq!(
            add_uri("a-b_c.d~e%f"),
            "mpdcontrol://add%20%22a-b_c.d~e%25f%22"
        );
    }

    #[test]
    fn add_uri_survives_a_round_trip_through_the_url_scheme() {
        // The half this module owns has to come back out of the half
        // `main::decode_command_from_url` + `command::parse` own — the two
        // sides are only connected by this format.
        let path = r#"Albums/Q/Queen/Weird "Quoted" & \Slashed\ Album/01 Track.mp3"#;
        let uri = url::Url::parse(&add_uri(path)).unwrap();
        let decoded = percent_encoding::percent_decode_str(uri.host_str().unwrap())
            .decode_utf8()
            .unwrap();
        assert_eq!(shlex::split(&decoded).unwrap(), vec!["add", path]);
    }

    #[test]
    fn album_copy_row_is_absent_for_a_library_root_file() {
        // No parent directory means no album to add — same rule as `album_key`.
        let queue = vec![queue_entry("loose_track.mp3", None, 1)];
        let model = model(
            PlayState::Playing,
            false,
            &queue,
            Some(CurrentTrack {
                pos: 0,
                next_pos: None,
                url: &queue[0].url,
                title: Some("Loose Track"),
                artists: &[],
            }),
            &[],
            &[],
            60,
        );
        assert!(model.album_title.is_none());
        assert!(model.track.is_some());
    }

    #[test]
    fn album_copy_row_labels_from_the_tag_but_links_the_directory() {
        // The label follows album-nav's naming (tag first), while the URI must
        // use the on-disk path — `add` takes a library path, not a tag.
        let queue = vec![queue_entry("A/ar 1972/1.mp3", Some("Abbey Road"), 1)];
        let row = album_copy_row(
            &queue,
            &CurrentTrack {
                pos: 0,
                next_pos: None,
                url: &queue[0].url,
                title: None,
                artists: &[],
            },
        )
        .unwrap();
        assert_eq!(row.label, "Abbey Road");
        assert_eq!(row.uri, add_uri("A/ar 1972"));
    }

    #[test]
    fn copy_id_round_trips_a_uri() {
        // URIs contain ':' (the scheme's own) — the parser must return the
        // whole remainder, not stop at the first colon.
        let uri = add_uri("Albums/A/Album/01 Track.mp3");
        assert_eq!(strip_copy_id(&copy_item_id(&uri)), Some(uri.as_str()));
    }

    #[test]
    fn addto_id_round_trips_name_containing_a_colon() {
        // Playlist names may contain ':' — the parser must recover
        // the whole remainder, not just up to the first colon.
        let name = "Side A: Favourites";
        assert_eq!(strip_addto_id(&addto_item_id(name)), Some(name));
    }

    #[test]
    fn album_id_round_trips_song_id() {
        let recovered = strip_album_id(&album_jump_item_id(42)).map(str::parse::<u64>);
        assert_eq!(recovered, Some(Ok(42)));
    }

    #[test]
    fn flag_payload_round_trips_every_flag_and_value() {
        for flag in [
            AlbumFlag::Relisten,
            AlbumFlag::Classical,
            AlbumFlag::Spoken,
            AlbumFlag::Soundtrack,
        ] {
            for value in [false, true] {
                let id = flag_item_id(flag, value);
                let payload = strip_flag_id(&id).unwrap();
                assert_eq!(parse_flag_payload(payload), Some((flag, value)));
            }
        }
    }

    #[test]
    fn parse_flag_payload_rejects_malformed_input() {
        assert_eq!(parse_flag_payload("classical"), None); // no ':'
        assert_eq!(parse_flag_payload("unknown:1"), None); // bad name
        assert_eq!(parse_flag_payload("classical:2"), None); // bad value
        assert_eq!(parse_flag_payload("classical:"), None);
    }

    #[test]
    fn id_parsers_reject_the_other_family_and_static_ids() {
        // What stops dispatch firing the wrong branch: each parser must not
        // claim ids belonging to the other family or to a static item.
        assert_eq!(strip_album_id(&addto_item_id("Five stars")), None);
        assert_eq!(strip_addto_id(&album_jump_item_id(42)), None);
        assert_eq!(strip_album_id(QUIT_ID), None);
        assert_eq!(strip_addto_id(QUIT_ID), None);

        let copy_id = copy_item_id(&add_uri("Al/1.mp3"));
        assert_eq!(strip_album_id(&copy_id), None);
        assert_eq!(strip_addto_id(&copy_id), None);
        assert_eq!(strip_copy_id(&addto_item_id("Five stars")), None);
        assert_eq!(strip_copy_id(&album_jump_item_id(42)), None);
        assert_eq!(strip_copy_id(QUIT_ID), None);

        let rating_id = rating_item_id(4);
        let flag_id = flag_item_id(AlbumFlag::Classical, true);
        assert_eq!(strip_album_id(&rating_id), None);
        assert_eq!(strip_addto_id(&rating_id), None);
        assert_eq!(strip_copy_id(&rating_id), None);
        assert_eq!(strip_flag_id(&rating_id), None);
        assert_eq!(strip_album_id(&flag_id), None);
        assert_eq!(strip_addto_id(&flag_id), None);
        assert_eq!(strip_copy_id(&flag_id), None);
        assert_eq!(strip_rating_id(&flag_id), None);
        assert_eq!(strip_rating_id(QUIT_ID), None);
        assert_eq!(strip_flag_id(QUIT_ID), None);
    }

    #[test]
    fn stars_renders_filled_then_empty() {
        assert_eq!(stars(0), "☆☆☆☆☆");
        assert_eq!(stars(1), "★☆☆☆☆");
        assert_eq!(stars(2), "★★☆☆☆");
        assert_eq!(stars(3), "★★★☆☆");
        assert_eq!(stars(4), "★★★★☆");
        assert_eq!(stars(5), "★★★★★");
    }

    fn info(
        year: Option<&str>,
        rating: Option<u8>,
        relisten: bool,
        classical: bool,
        spoken: bool,
        soundtrack: bool,
    ) -> AlbumInfo {
        AlbumInfo {
            year: year.map(str::to_string),
            rating,
            relisten,
            classical,
            spoken,
            soundtrack,
        }
    }

    #[test]
    fn album_summary_worked_examples() {
        // Matches the rendered examples in docs/usage.md, in order.
        assert_eq!(
            album_summary(&info(Some("1996"), Some(4), true, false, false, true)),
            Some("1996 · ★★★★☆ · ↻ · Soundtrack".to_string())
        );
        assert_eq!(
            album_summary(&info(Some("1996"), Some(3), false, false, false, false)),
            Some("1996 · ★★★☆☆".to_string())
        );
        assert_eq!(
            album_summary(&info(Some("2003"), Some(2), false, true, true, false)),
            Some("2003 · ★★☆☆☆ · Classical · Spoken".to_string())
        );
        assert_eq!(
            album_summary(&info(None, None, true, true, false, false)),
            Some("↻ · Classical".to_string())
        );
        // Unrated, no year, no flags: the whole row is omitted, not emitted empty.
        assert_eq!(
            album_summary(&info(None, None, false, false, false, false)),
            None
        );
    }

    #[test]
    fn album_summary_rating_zero_is_distinct_from_no_rating() {
        // `rating = 0` renders as all-empty stars — visually distinct from
        // the rating part being absent entirely.
        assert_eq!(
            album_summary(&info(None, Some(0), false, false, false, false)),
            Some("☆☆☆☆☆".to_string())
        );
    }

    #[test]
    fn album_section_no_album_hides_everything() {
        let section = album_section(&AlbumState::NoAlbum, None);
        assert_eq!(section.summary, None);
        assert_eq!(section.notice, None);
        assert_eq!(section.controls, None);
    }

    #[test]
    fn album_section_unavailable_shows_notice_only() {
        let section = album_section(&AlbumState::Unavailable, None);
        assert_eq!(section.summary, None);
        assert_eq!(section.notice, Some(UNAVAILABLE_NOTICE.to_string()));
        assert_eq!(section.controls, None);
    }

    #[test]
    fn album_section_not_found_shows_summary_only() {
        let section = album_section(&AlbumState::NotFound, None);
        assert_eq!(section.summary, Some(NOT_FOUND_SUMMARY.to_string()));
        assert_eq!(section.notice, None);
        assert_eq!(section.controls, None);
    }

    #[test]
    fn album_section_found_yields_summary_and_controls() {
        let album_info = info(Some("1996"), Some(4), false, false, false, false);
        let section = album_section(&AlbumState::Found(album_info), None);
        assert_eq!(section.summary, Some("1996 · ★★★★☆".to_string()));
        assert_eq!(section.notice, None);
        assert_eq!(
            section.controls,
            Some(info(Some("1996"), Some(4), false, false, false, false))
        );
    }

    #[test]
    fn album_section_write_error_overrides_the_notice_but_not_summary_or_controls() {
        // A failed write on an otherwise `Found` row still shows the row's
        // real (unchanged) summary and controls — the notice is additional,
        // so a failed write is self-evident as "the tick I clicked isn't
        // there" rather than a misleading optimistic checkmark.
        let album_info = info(Some("1996"), Some(4), false, false, false, false);
        let section = album_section(&AlbumState::Found(album_info), Some("disk full"));
        assert_eq!(section.notice, Some(WRITE_FAILED_NOTICE.to_string()));
        assert!(section.summary.is_some());
        assert!(section.controls.is_some());

        // It also takes priority over the plain "unavailable" notice.
        let unavailable = album_section(&AlbumState::Unavailable, Some("disk full"));
        assert_eq!(unavailable.notice, Some(WRITE_FAILED_NOTICE.to_string()));
    }

    #[test]
    fn rating_checked_matches_only_the_stored_rating() {
        let unrated = info(None, None, false, false, false, false);
        for r in 0..=5 {
            assert!(!rating_checked(&unrated, r));
        }

        let rated = AlbumInfo {
            rating: Some(3),
            ..unrated
        };
        for r in 0..=5 {
            assert_eq!(rating_checked(&rated, r), r == 3);
        }
    }

    #[test]
    fn show_track_section_true_with_only_album_data() {
        let mut model = model_with(None, None, vec![], vec![]);
        model.album_section = AlbumSection {
            summary: Some("1996 · ★★★★☆".to_string()),
            notice: None,
            controls: None,
        };
        assert!(model.show_track_section());
    }

    #[test]
    fn show_track_section_true_with_notice_only() {
        // A notice-only section (e.g. "album database unavailable") must
        // still surface the track section — it's the only thing telling the
        // user the feature is broken.
        let mut model = model_with(None, None, vec![], vec![]);
        model.album_section = AlbumSection {
            summary: None,
            notice: Some(UNAVAILABLE_NOTICE.to_string()),
            controls: None,
        };
        assert!(model.show_track_section());
    }
}
