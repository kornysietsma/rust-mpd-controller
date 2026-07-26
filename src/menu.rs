use anyhow::Result;
use mpd_client::responses::PlayState;
use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};

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
/// occurred so a disabled dropdown item can show it in full. When `state` is
/// `Paused`, the paused glyph is prefixed onto `title` *after* truncation, so
/// it sits outside the 25-char budget and never affects where the text is
/// cut (spec §7.1). `full_text` (the untruncated dropdown text) never gets
/// the prefix — it's not the tray title.
pub fn track_display(
    state: PlayState,
    title: Option<&str>,
    url: &str,
    artists: &[String],
) -> Display {
    let full = format_track(title, url, artists);
    let truncated = truncate(&full, TITLE_MAX_CHARS);
    let full_text = if truncated != full { Some(full) } else { None };
    let title = if state == PlayState::Paused {
        format!("{PAUSED_ICON} {truncated}")
    } else {
        truncated
    };
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
        Some(song) => track_display(
            snapshot.status.state,
            song.title(),
            &song.url,
            song.artists(),
        ),
        None => empty_queue_display(),
    }
}

/// "In playlists" contents (spec §6.1): every playlist containing the
/// current song, sorted case-insensitively.
fn in_playlists_list(playlists_for_url: &[String]) -> Vec<String> {
    let mut names = playlists_for_url.to_vec();
    names.sort_by_key(|name| name.to_lowercase());
    names
}

/// "Add to" contents (spec §6.2): `key_playlists` minus whichever already
/// contain the song, preserving config order. Trusts `key_playlists` to
/// already be validated against real MPD playlist names — whether an entry
/// corresponds to a real playlist is decided upstream (spec §3, the caller
/// filters against `PlaylistCache::names` before this point), not here.
fn add_to_list(playlists_for_url: &[String], key_playlists: &[String]) -> Vec<String> {
    key_playlists
        .iter()
        .filter(|name| !playlists_for_url.contains(name))
        .cloned()
        .collect()
}

/// Whether album-nav should be computed at all, folding in the "nothing
/// playing" condition from spec §5.3 that sits outside `show_album_nav`'s
/// own remit (that function only looks at `random`/`run_len`).
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

/// Everything needed to render the dropdown menu, computed from primitives
/// so it's unit-testable without a live MPD connection (spec §8.1). No
/// `title` field — the tray title is set independently via `Display`
/// (`display_for_snapshot`); spec §8.1 includes one, but nothing consumes it
/// here, so it's left out (YAGNI) and can be added back if a real need shows
/// up.
pub struct MenuModel {
    pub full_text: Option<String>,
    pub prev_album: Option<AlbumJump>,
    pub next_album: Option<AlbumJump>,
    pub in_playlists: Vec<String>,
    pub add_to: Vec<String>,
}

impl MenuModel {
    /// Whether the album-nav section (and the separator preceding it) should
    /// be emitted at all. Spec §6's "omit the separator when the section
    /// between them is empty" rule — a decision, so it lives here rather than
    /// in `render`, which can't be tested (see the module note on
    /// `muda::Menu` being main-thread-only). Derived rather than stored, so
    /// it can't drift from the fields it's computed from.
    pub fn show_album_section(&self) -> bool {
        self.prev_album.is_some() || self.next_album.is_some()
    }

    /// Whether the current-track section (full text, "In playlists", "Add
    /// to") and its preceding separator should be emitted. Same rule and same
    /// reasoning as `show_album_section`.
    pub fn show_track_section(&self) -> bool {
        self.full_text.is_some() || !self.in_playlists.is_empty() || !self.add_to.is_empty()
    }
}

/// Pure: assembles a `MenuModel` from already-extracted primitives. All the
/// rules in spec §5 and §6 live here; `render` (step 7) makes no further
/// decisions.
#[allow(clippy::too_many_arguments)]
pub fn model(
    state: PlayState,
    random: bool,
    current: Option<&QueueEntry>,
    current_pos: Option<usize>,
    queue: &[QueueEntry],
    playlists_for_url: &[String],
    key_playlists: &[String],
    title: Option<&str>,
    artists: &[String],
) -> MenuModel {
    let full_text =
        current.and_then(|entry| track_display(state, title, &entry.url, artists).full_text);
    let (prev_album, next_album) = album_nav(state, random, queue, current_pos);

    MenuModel {
        full_text,
        prev_album,
        next_album,
        in_playlists: in_playlists_list(playlists_for_url),
        add_to: add_to_list(playlists_for_url, key_playlists),
    }
}

// All glyphs are pinned to text presentation with U+FE0E (spec §7) so they
// render monochrome and narrow, matching the menu-bar font weight, rather
// than falling back to wide colour emoji.
const PLAY_PAUSE_ICON: &str = "\u{23EF}\u{FE0E}";
const NEXT_ICON: &str = "\u{23ED}\u{FE0E}";
const CLEAR_ICON: &str = "\u{23F9}\u{FE0E}";
const PREV_ALBUM_ICON: &str = "\u{23EE}\u{FE0E}\u{23EE}\u{FE0E}";
const NEXT_ALBUM_ICON: &str = "\u{23ED}\u{FE0E}\u{23ED}\u{FE0E}";
/// Prefixed onto the tray title when paused, by `track_display` above — a
/// `Display` concern, not used by `render` below.
const PAUSED_ICON: &str = "\u{23F8}\u{FE0E}";

/// The id for an album-navigation menu item (spec §8.2): `album:<song_id>`.
fn album_jump_item_id(song_id: u64) -> String {
    format!("album:{song_id}")
}

/// The id for an "Add to" playlist menu item (spec §8.2): `addto:<name>`.
/// Playlist names may contain `:`; dispatch (step 10) recovers the full name
/// via `strip_prefix("addto:")`, which doesn't split on interior colons.
fn addto_item_id(name: &str) -> String {
    format!("addto:{name}")
}

/// Turns an already-computed `MenuModel` into `muda` items. Every rule from
/// spec §5/§6 lives in `model()` and `MenuModel`'s `show_*_section` methods,
/// which is what keeps them testable: **this function cannot be unit-tested**,
/// because `muda::Menu::new` panics off the main thread and `cargo test` runs
/// tests on spawned threads. Keep decisions out of here.
pub fn render(model: &MenuModel) -> Result<Menu> {
    let menu = Menu::new();

    menu.append(&MenuItem::with_id(
        PLAY_PAUSE_ID,
        format!("{PLAY_PAUSE_ICON} Play/Pause"),
        true,
        None,
    ))?;
    menu.append(&MenuItem::with_id(
        NEXT_ID,
        format!("{NEXT_ICON} Next"),
        true,
        None,
    ))?;
    menu.append(&MenuItem::with_id(
        CLEAR_ID,
        format!("{CLEAR_ICON} Clear"),
        true,
        None,
    ))?;

    if model.show_album_section() {
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
        if let Some(text) = &model.full_text {
            menu.append(&MenuItem::new(text, false, None))?;
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
        let display = track_display(PlayState::Playing, Some("My Title"), "foo.mp3", &artists);
        assert_eq!(display.title, "My Title - My Artist");
        assert!(display.full_text.is_none());
    }

    #[test]
    fn track_display_truncates_and_exposes_full_text() {
        let long_title = "a".repeat(40);
        let display = track_display(PlayState::Playing, Some(&long_title), "foo.mp3", &[]);
        assert_eq!(display.title.chars().count(), TITLE_MAX_CHARS);
        assert_eq!(display.full_text.as_deref(), Some(long_title.as_str()));
    }

    #[test]
    fn track_display_prefixes_paused_glyph_when_paused() {
        let display = track_display(PlayState::Paused, Some("My Title"), "foo.mp3", &[]);
        assert_eq!(display.title, format!("{PAUSED_ICON} My Title"));
    }

    #[test]
    fn track_display_omits_paused_glyph_when_playing_or_stopped() {
        let playing = track_display(PlayState::Playing, Some("My Title"), "foo.mp3", &[]);
        assert_eq!(playing.title, "My Title");
        let stopped = track_display(PlayState::Stopped, Some("My Title"), "foo.mp3", &[]);
        assert_eq!(stopped.title, "My Title");
    }

    #[test]
    fn track_display_paused_prefix_sits_outside_the_truncation_budget() {
        // A title that exactly fills the 25-char budget must truncate
        // identically whether paused or playing (spec §7.1) — the glyph is
        // prepended after truncation, so it doesn't eat into the budget.
        let title = "a".repeat(TITLE_MAX_CHARS);
        let playing = track_display(PlayState::Playing, Some(&title), "foo.mp3", &[]);
        let paused = track_display(PlayState::Paused, Some(&title), "foo.mp3", &[]);
        assert_eq!(playing.title, title);
        assert_eq!(paused.title, format!("{PAUSED_ICON} {title}"));
        assert!(playing.full_text.is_none());
        assert!(paused.full_text.is_none());
    }

    #[test]
    fn track_display_long_title_truncates_identically_paused_or_playing() {
        let long_title = "a".repeat(40);
        let playing = track_display(PlayState::Playing, Some(&long_title), "foo.mp3", &[]);
        let paused = track_display(PlayState::Paused, Some(&long_title), "foo.mp3", &[]);
        assert_eq!(paused.title, format!("{PAUSED_ICON} {}", playing.title));
        assert_eq!(playing.full_text, paused.full_text);
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
    fn add_to_list_empty_key_playlists_is_empty() {
        let playlists_for_url = vec!["Five stars".to_string()];
        assert_eq!(add_to_list(&playlists_for_url, &[]), Vec::<String>::new());
    }

    #[test]
    fn add_to_list_song_in_no_playlists_offers_full_key_playlists_in_order() {
        let key_playlists = vec![
            "Five stars".to_string(),
            "My Top Rated".to_string(),
            "mellow_2024".to_string(),
        ];
        assert_eq!(in_playlists_list(&[]), Vec::<String>::new());
        assert_eq!(add_to_list(&[], &key_playlists), key_playlists);
    }

    #[test]
    fn add_to_list_does_not_itself_validate_playlist_existence() {
        // Whether a `key_playlists` entry corresponds to a real MPD playlist
        // is resolved upstream of this function (main.rs filters
        // `key_playlists` against `PlaylistCache::names` before calling
        // `model()`, spec §3). From here, an unresolvable name is
        // indistinguishable from a real playlist that just doesn't contain
        // this song, and is included the same way — pinning that this
        // function needs no existence-check of its own.
        let playlists_for_url = vec!["Five stars".to_string()];
        let key_playlists = vec!["Five stars".to_string(), "Typo'd Playlist".to_string()];
        assert_eq!(
            add_to_list(&playlists_for_url, &key_playlists),
            vec!["Typo'd Playlist".to_string()]
        );
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
            Some(&queue[2]),
            Some(2),
            &queue,
            &[],
            &[],
            Some("Track 3"),
            &[],
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
            Some(&queue[2]),
            Some(2),
            &queue,
            &playlists_for_url,
            &key_playlists,
            Some("Come Together"),
            &artists,
        );

        assert_eq!(
            model.full_text.as_deref(),
            Some("Come Together - The Beatles (Remastered 2019 Edition)")
        );
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
        full_text: Option<&str>,
        prev_album: Option<AlbumJump>,
        in_playlists: Vec<String>,
        add_to: Vec<String>,
    ) -> MenuModel {
        MenuModel {
            full_text: full_text.map(str::to_string),
            prev_album,
            next_album: None,
            in_playlists,
            add_to,
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
        // spec §6's "never two adjacent separators".
        let model = model_with(None, None, vec![], vec![]);
        assert!(!model.show_album_section());
        assert!(!model.show_track_section());
    }

    #[test]
    fn album_section_shows_when_either_jump_is_present() {
        assert!(model_with(None, Some(a_jump()), vec![], vec![]).show_album_section());

        let next_only = MenuModel {
            next_album: Some(a_jump()),
            ..model_with(None, None, vec![], vec![])
        };
        assert!(next_only.show_album_section());
    }

    #[test]
    fn track_section_shows_when_any_of_its_three_parts_is_present() {
        assert!(model_with(Some("long track text"), None, vec![], vec![]).show_track_section());
        assert!(
            model_with(None, None, vec!["Five stars".to_string()], vec![]).show_track_section()
        );
        assert!(
            model_with(None, None, vec![], vec!["My Top Rated".to_string()]).show_track_section()
        );
    }

    #[test]
    fn addto_item_id_round_trips_name_containing_a_colon() {
        // Playlist names may contain ':' (spec §8.2) — `strip_prefix` must
        // recover the whole remainder, not just up to the first colon.
        let name = "Side A: Favourites";
        let id = addto_item_id(name);
        assert_eq!(id.strip_prefix("addto:"), Some(name));
    }

    #[test]
    fn album_jump_item_id_round_trips_song_id() {
        let id = album_jump_item_id(42);
        let recovered: u64 = id.strip_prefix("album:").unwrap().parse().unwrap();
        assert_eq!(recovered, 42);
    }
}
