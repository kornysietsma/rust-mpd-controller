use anyhow::Result;
use tray_icon::menu::{Menu, MenuItem};

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
    pub id: u64,
}

/// The parent directory of a song URL, used as its album identity (spec
/// §5.1): two queue entries are the same album iff they share one. `None`
/// for a library-root file with no parent directory.
fn album_key(url: &str) -> Option<&str> {
    url.rsplit_once('/').map(|(dir, _)| dir)
}

/// Whether two queue entries belong to the same album — i.e. both have a
/// parent directory, and it's the same one. A library-root file (no parent)
/// never counts as sharing an album with anything, including another
/// root-level file.
fn same_album(a: &str, b: &str) -> bool {
    matches!((album_key(a), album_key(b)), (Some(x), Some(y)) if x == y)
}

/// The maximal contiguous range of queue positions, containing `current_pos`,
/// that share `current_pos`'s album (spec §5.3/§5.4). Scans outward from
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

/// Whether album-nav rows should be shown at all (spec §5.3): only while not
/// in `random` mode and only when sitting inside a run of at least 5. The
/// "nothing playing / stopped" case isn't this function's concern — the
/// caller only calls it when there's a current position to evaluate.
fn show_album_nav(random: bool, run_len: usize) -> bool {
    !random && run_len >= ALBUM_NAV_MIN_RUN
}

const ALBUM_NAV_MIN_RUN: usize = 5;
const ALBUM_NAME_MAX_CHARS: usize = 40;

/// An album-navigation jump target (spec §5.4): the song to land on, and the
/// full menu label describing the destination (e.g. `"Next Album: Let It
/// Be"` or `"Start of Album"`). `render()` and click dispatch need no
/// further knowledge of which case produced it.
#[derive(Debug, PartialEq, Eq)]
pub struct AlbumJump {
    pub label: String,
    pub song_id: u64,
}

/// The basename of a song's parent directory — the fallback album display
/// name when the `album` tag is absent (spec §5.2). `None` for a
/// library-root file with no parent directory.
fn directory_basename(url: &str) -> Option<&str> {
    let dir = album_key(url)?;
    Some(dir.rsplit('/').next().unwrap_or(dir))
}

/// The display name for the album a queue entry belongs to: its `album` tag,
/// falling back to the parent directory's basename, then to the raw URL if
/// even that's unavailable (a library-root file). Truncated to 40 chars
/// (spec §5.2) — applied to the name only, so a long title never pushes the
/// `Next Album: ` prefix off the menu.
fn album_display_name(entry: &QueueEntry) -> String {
    let name = entry
        .album
        .clone()
        .or_else(|| directory_basename(&entry.url).map(str::to_string))
        .unwrap_or_else(|| entry.url.clone());
    truncate(&name, ALBUM_NAME_MAX_CHARS)
}

/// Next-album target (spec §5.4): the run immediately following the current
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

/// Prev/Start-of-Album target (spec §5.4): two states depending on where
/// `current_pos` sits within its run.
/// - Track 2+ (`current_pos > run_start`): jump to the run's own first
///   entry, labelled `"Start of Album"` (no name — the run is already
///   playing, so a name would be redundant).
/// - Track 1 (`current_pos == run_start`): jump to the *preceding* run's
///   first entry, labelled `"Prev Album: <name>"`. `None` if there is no
///   preceding run (this is the first album in the queue).
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
const TITLE_MAX_CHARS: usize = 25;
const ELLIPSIS: char = '…';

/// What the tray should show: the (possibly truncated) title text, and, if
/// truncation occurred, the untruncated text for a disabled dropdown item.
pub struct Display {
    pub title: String,
    pub full_text: Option<String>,
}

/// Formats a song as "Title - Artist". Falls back to the song's
/// URL when there's no title tag, and omits the artist half when there are no
/// artist tags. Takes already-extracted primitives (rather than mpd_client's
/// `Song`) so this pure logic is unit-testable without a live connection.
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

/// Computes the tray display for a song that's playing/paused:
/// the truncated track text, plus the untruncated text when truncation
/// occurred so a disabled dropdown item can show it in full.
pub fn track_display(title: Option<&str>, url: &str, artists: &[String]) -> Display {
    let full = format_track(title, url, artists);
    let title = truncate(&full, TITLE_MAX_CHARS);
    let full_text = if title != full { Some(full) } else { None };
    Display { title, full_text }
}

/// The tray display when connected but nothing is playing.
pub fn empty_queue_display() -> Display {
    Display {
        title: EMPTY_QUEUE_ICON.to_string(),
        full_text: None,
    }
}

/// The tray display while MPD is unreachable.
pub fn disconnected_display() -> Display {
    Display {
        title: DISCONNECTED_ICON.to_string(),
        full_text: None,
    }
}

/// Computes the tray display for a connected snapshot: track text when a
/// song is playing/paused, the "nothing playing" icon otherwise.
pub fn display_for_snapshot(snapshot: &PlayerSnapshot) -> Display {
    match &snapshot.song {
        Some(song) => track_display(song.title(), &song.url, song.artists()),
        None => empty_queue_display(),
    }
}

/// Builds a fresh dropdown menu: `Play/Pause`, `Next`, `Clear`, an
/// optional disabled item showing the untruncated title (only present when
/// the tray title above it was truncated), then `Quit`.
pub fn build_menu(full_text: Option<&str>) -> Result<Menu> {
    let menu = Menu::new();
    menu.append(&MenuItem::with_id(PLAY_PAUSE_ID, "Play/Pause", true, None))?;
    menu.append(&MenuItem::with_id(NEXT_ID, "Next", true, None))?;
    menu.append(&MenuItem::with_id(CLEAR_ID, "Clear", true, None))?;
    if let Some(text) = full_text {
        menu.append(&MenuItem::new(text, false, None))?;
    }
    menu.append(&MenuItem::with_id(QUIT_ID, "Quit", true, None))?;
    Ok(menu)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn format_track_combines_title_and_artist() {
        let artists = vec!["My Artist".to_string()];
        assert_eq!(
            format_track(Some("My Title"), "foo.mp3", &artists),
            "My Title - My Artist"
        );
    }

    #[test]
    fn format_track_joins_multiple_artists() {
        let artists = vec!["Artist A".to_string(), "Artist B".to_string()];
        assert_eq!(
            format_track(Some("My Title"), "foo.mp3", &artists),
            "My Title - Artist A, Artist B"
        );
    }

    #[test]
    fn format_track_omits_artist_when_absent() {
        assert_eq!(format_track(Some("My Title"), "foo.mp3", &[]), "My Title");
    }

    #[test]
    fn format_track_falls_back_to_url_when_no_title() {
        let artists = vec!["My Artist".to_string()];
        assert_eq!(
            format_track(None, "Album/track.mp3", &artists),
            "Album/track.mp3 - My Artist"
        );
    }

    #[test]
    fn truncate_leaves_short_text_untouched() {
        assert_eq!(truncate("short", 25), "short");
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
    fn truncate_is_unicode_safe() {
        // Multi-byte characters throughout; must not panic or split a char.
        let text = "🎵".repeat(30);
        let result = truncate(&text, 25);
        assert_eq!(result.chars().count(), 25);
        assert!(result.ends_with(ELLIPSIS));
    }

    #[test]
    fn track_display_shows_full_text_untruncated_when_short() {
        let artists = vec!["My Artist".to_string()];
        let display = track_display(Some("My Title"), "foo.mp3", &artists);
        assert_eq!(display.title, "My Title - My Artist");
        assert!(display.full_text.is_none());
    }

    #[test]
    fn track_display_truncates_and_exposes_full_text() {
        let long_title = "a".repeat(40);
        let display = track_display(Some(&long_title), "foo.mp3", &[]);
        assert_eq!(display.title.chars().count(), TITLE_MAX_CHARS);
        assert_eq!(display.full_text.as_deref(), Some(long_title.as_str()));
    }

    #[test]
    fn empty_queue_display_shows_icon() {
        let display = empty_queue_display();
        assert_eq!(display.title, EMPTY_QUEUE_ICON);
        assert!(display.full_text.is_none());
    }

    #[test]
    fn disconnected_display_shows_icon() {
        let display = disconnected_display();
        assert_eq!(display.title, DISCONNECTED_ICON);
        assert!(display.full_text.is_none());
    }

    fn entry(url: &str) -> QueueEntry {
        QueueEntry {
            url: url.to_string(),
            album: None,
            id: 0,
        }
    }

    #[test]
    fn album_key_same_directory_is_same_album() {
        assert_eq!(
            album_key("Beatles/Abbey Road/01 Come Together.mp3"),
            album_key("Beatles/Abbey Road/02 Something.mp3")
        );
    }

    #[test]
    fn album_key_different_directory_is_different_album() {
        assert_ne!(
            album_key("Beatles/Abbey Road/01 Come Together.mp3"),
            album_key("Beatles/Let It Be/01 Two of Us.mp3")
        );
    }

    #[test]
    fn album_key_multi_disc_in_one_directory_is_one_album() {
        // Continuous track numbering across a two-disc album living in a
        // single directory (spec §2.1) — no CD1/CD2 subdirectories.
        assert_eq!(
            album_key("Pink Floyd/The Wall/12 Goodbye Cruel World.mp3"),
            album_key("Pink Floyd/The Wall/13 Hey You.mp3")
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
    fn album_run_empty_queue_is_none() {
        assert_eq!(album_run(&[], 0), None);
    }

    #[test]
    fn album_run_single_entry_queue() {
        let queue = vec![entry("A/Al/1.mp3")];
        assert_eq!(album_run(&queue, 0), Some((0, 0)));
    }

    #[test]
    fn album_run_out_of_bounds_position_is_none() {
        let queue = vec![entry("A/Al/1.mp3"), entry("A/Al/2.mp3")];
        assert_eq!(album_run(&queue, 5), None);
    }

    fn queue_entry(url: &str, album: Option<&str>, id: u64) -> QueueEntry {
        QueueEntry {
            url: url.to_string(),
            album: album.map(str::to_string),
            id,
        }
    }

    #[test]
    fn show_album_nav_run_of_five_shown_run_of_four_hidden() {
        assert!(show_album_nav(false, 5));
        assert!(!show_album_nav(false, 4));
    }

    #[test]
    fn show_album_nav_hidden_when_random_regardless_of_run_len() {
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
    fn album_display_name_truncates_long_title() {
        let long_title = "a".repeat(192);
        let entry = queue_entry("Beatles/Abbey Road/1.mp3", Some(&long_title), 1);
        assert_eq!(album_display_name(&entry).chars().count(), 40);
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
    fn prev_album_jump_on_last_track_of_run_still_returns_start_of_album() {
        let queue = vec![
            queue_entry("A/Abbey Road/1.mp3", Some("Abbey Road"), 1),
            queue_entry("A/Abbey Road/2.mp3", Some("Abbey Road"), 2),
            queue_entry("A/Abbey Road/3.mp3", Some("Abbey Road"), 3),
        ];
        let jump = prev_album_jump(&queue, 2).unwrap();
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
    fn prev_album_jump_at_track_two_of_first_album_offers_start_of_album() {
        let queue = vec![
            queue_entry("A/Abbey Road/1.mp3", Some("Abbey Road"), 1),
            queue_entry("A/Abbey Road/2.mp3", Some("Abbey Road"), 2),
        ];
        let jump = prev_album_jump(&queue, 1).unwrap();
        assert_eq!(jump.song_id, 1);
        assert_eq!(jump.label, "Start of Album");
    }

    #[test]
    fn traversal_never_wraps_regardless_of_repeat() {
        // Neither traversal function takes a `repeat` parameter — there's
        // nothing to consult, so wraparound can't happen no matter MPD's
        // repeat setting.
        let queue = vec![
            queue_entry("A/Abbey Road/1.mp3", Some("Abbey Road"), 1),
            queue_entry("A/Abbey Road/2.mp3", Some("Abbey Road"), 2),
        ];
        assert_eq!(next_album_jump(&queue, 1), None);
        assert_eq!(prev_album_jump(&queue, 0), None);
    }
}
