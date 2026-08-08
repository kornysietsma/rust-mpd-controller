#[cfg(test)]
use std::fs;
#[cfg(test)]
use std::path::Path;

use albumdb::SongTags;
use anyhow::{Context, Result};
use mpd_client::client::Client;
use mpd_client::commands::Command as MpdCommand;
use mpd_client::protocol::command::Command as RawCommand;
use mpd_client::protocol::response::Frame;
use mpd_client::responses::TypedResponseError;
use tokio::net::TcpStream;

use crate::mpd_wire::{DirListing, base_filter, parse_directory_listing};

/// Produces the full set of song tags backing the album database: the MPD I/O
/// layer's only job. Everything downstream — grouping into albums,
/// `tag_state`, title/artist resolution — is a pure function over the
/// `Vec<SongTags>` this returns.
///
/// Deliberately synchronous: `mpd_client`'s async API is real, but nothing
/// about reading the library benefits from being async at this trait
/// boundary — it's one batch read, not a long-lived connection. The real
/// implementation owns a private Tokio runtime for the duration of the call.
/// `sync`'s `MusicBrainzClient` is async instead, since its rate-limited,
/// request-per-album loop is a much more natural fit for it.
pub trait LibrarySource {
    fn songs(&self) -> Result<Vec<SongTags>>;
}

/// Reads the MPD library over TCP. `find "(base '')"` over the whole library
/// returns zero rows — the response is too large — so the walk is chunked:
/// `listfiles` for top-level directories, `lsinfo` per top-level directory
/// for chunk boundaries, then `find "(base '<chunk>')"` per chunk.
pub struct MpdLibrarySource {
    host: String,
    port: u16,
}

impl MpdLibrarySource {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
        }
    }

    async fn songs_async(&self) -> Result<Vec<SongTags>> {
        let stream = TcpStream::connect((self.host.as_str(), self.port))
            .await
            .with_context(|| {
                format!(
                    "failed to open TCP connection to {}:{}",
                    self.host, self.port
                )
            })?;
        let (client, _events) = Client::connect(stream)
            .await
            .context("MPD handshake failed")?;

        let mut songs = Vec::new();
        let mut seen_paths = std::collections::HashSet::new();
        let mut push_all = |listing_songs: Vec<SongTags>, songs: &mut Vec<SongTags>| {
            for song in listing_songs {
                if seen_paths.insert(song.path.clone()) {
                    songs.push(song);
                }
            }
        };

        let top_dirs = client
            .command(ListFiles)
            .await
            .context("failed to list top-level MPD directories")?;
        for top_dir in &top_dirs {
            let sub = client
                .command(LsInfo(top_dir))
                .await
                .with_context(|| format!("failed to lsinfo {top_dir:?}"))?;
            push_all(sub.songs, &mut songs);

            for chunk in &sub.directories {
                let found = client
                    .command(FindBase(chunk))
                    .await
                    .with_context(|| format!("failed to find songs under {chunk:?}"))?;
                push_all(found.songs, &mut songs);
            }
        }

        Ok(songs)
    }
}

impl LibrarySource for MpdLibrarySource {
    fn songs(&self) -> Result<Vec<SongTags>> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("failed to start async runtime for MPD connection")?;
        runtime.block_on(self.songs_async())
    }
}

/// `listfiles` with no argument — top-level directories. `lsinfo`/
/// `listallinfo` with no argument return zero rows and then drop the
/// connection; `listfiles` is the one that actually works here.
///
/// Only `directory` entries are kept. `listfiles` is a raw filesystem
/// browse rather than a database query — it lists `file` entries with no
/// tag data at all, for any file type MPD recognizes or not. Confirmed
/// live: the real music directory's true root holds 46 such stray
/// non-audio files, and treating them as songs over-counted the library
/// past MPD's own `stats` total. `lsinfo`, used one level down, returns
/// full tag data for loose *songs* and is where those are actually caught.
struct ListFiles;

impl MpdCommand for ListFiles {
    type Response = Vec<String>;

    fn command(&self) -> RawCommand {
        RawCommand::new("listfiles")
    }

    fn response(self, frame: Frame) -> Result<Self::Response, TypedResponseError> {
        Ok(frame
            .into_iter()
            .filter_map(|(k, v)| (k.as_ref() == "directory").then_some(v))
            .collect())
    }
}

/// `lsinfo <directory>` — one level of subdirectories, plus full tag data
/// for any loose files directly in `directory` (not nested in any of those
/// subdirectories).
struct LsInfo<'a>(&'a str);

impl MpdCommand for LsInfo<'_> {
    type Response = DirListing;

    fn command(&self) -> RawCommand {
        RawCommand::new("lsinfo").argument(self.0)
    }

    fn response(self, frame: Frame) -> Result<Self::Response, TypedResponseError> {
        Ok(parse_directory_listing(
            frame.into_iter().map(|(k, v)| (k.to_string(), v)),
        ))
    }
}

/// `find "(base '<path>')"` — every song beneath `path`, recursively. Never
/// returns `directory` fields (that's what makes it recursive), so
/// `DirListing::directories` is always empty for this command.
struct FindBase<'a>(&'a str);

impl MpdCommand for FindBase<'_> {
    type Response = DirListing;

    fn command(&self) -> RawCommand {
        RawCommand::new("find").argument(base_filter(self.0))
    }

    fn response(self, frame: Frame) -> Result<Self::Response, TypedResponseError> {
        Ok(parse_directory_listing(
            frame.into_iter().map(|(k, v)| (k.to_string(), v)),
        ))
    }
}

/// Reads a committed fixture file instead of a live MPD server — the
/// substitute this trait exists to allow. The file uses the same
/// `key: value` line shape MPD's own protocol uses, with a blank line between
/// songs, so [`parse_directory_listing`] (shared with the real
/// implementation) can parse it unchanged.
///
/// Test-only (like `albumdb::song_tags::song`): nothing in `album-tool`
/// itself ever reads a fixture outside `cargo test`.
#[cfg(test)]
pub(crate) struct FixtureLibrarySource {
    contents: String,
}

#[cfg(test)]
impl FixtureLibrarySource {
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("failed to read fixture library at {}", path.display()))?;
        Ok(Self { contents })
    }
}

#[cfg(test)]
impl LibrarySource for FixtureLibrarySource {
    fn songs(&self) -> Result<Vec<SongTags>> {
        let fields = self
            .contents
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(|line| {
                let (key, value) = line
                    .split_once(": ")
                    .with_context(|| format!("malformed fixture line: {line:?}"))?;
                Ok((key.to_string(), value.to_string()))
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(parse_directory_listing(fields).songs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The success path is covered by `scan`'s tests, which assert the exact
    // album and tag_state counts this fixture is built to produce.

    #[test]
    fn fixture_source_missing_file_is_a_clear_error() {
        let result = FixtureLibrarySource::load(Path::new("/nonexistent/library.txt"));
        assert!(result.is_err());
    }
}
