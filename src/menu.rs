use anyhow::Result;
use tray_icon::menu::{Menu, MenuItem};

use crate::mpd::PlayerSnapshot;

pub const PLAY_PAUSE_ID: &str = "play_pause";
pub const NEXT_ID: &str = "next";
pub const CLEAR_ID: &str = "clear";
pub const QUIT_ID: &str = "quit";

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

/// Formats a song as "Title - Artist", per spec §6. Falls back to the song's
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

/// Computes the tray display for a song that's playing/paused (spec §6):
/// the truncated track text, plus the untruncated text when truncation
/// occurred so a disabled dropdown item can show it in full.
pub fn track_display(title: Option<&str>, url: &str, artists: &[String]) -> Display {
    let full = format_track(title, url, artists);
    let title = truncate(&full, TITLE_MAX_CHARS);
    let full_text = if title != full { Some(full) } else { None };
    Display { title, full_text }
}

/// The tray display when connected but nothing is playing (spec §6).
pub fn empty_queue_display() -> Display {
    Display {
        title: EMPTY_QUEUE_ICON.to_string(),
        full_text: None,
    }
}

/// The tray display while MPD is unreachable (spec §6.1).
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

/// Builds a fresh dropdown menu (spec §6): `Play/Pause`, `Next`, `Clear`, an
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
}
