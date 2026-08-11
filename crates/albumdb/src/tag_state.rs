use std::collections::BTreeSet;

use crate::enums::TagState;
use crate::song_tags::{SongTags, present_tag as present};

/// First four *characters* of a date string. Slicing by byte index would
/// panic if a multi-byte character straddled index 4 — one mangled `Date` tag
/// anywhere in the library would abort the whole scan.
fn year_of(date: &str) -> &str {
    let end = date
        .char_indices()
        .nth(4)
        .map_or(date.len(), |(index, _)| index);
    &date[..end]
}

/// Derive `tag_state` for one album directory. Recomputed from scratch on
/// every scan, which is what lets an album re-tagged in Picard move
/// `no_mbid_* → clean` with no extra mechanism.
///
/// Two rules here are counter-intuitive enough to be worth stating (the full
/// taxonomy and its measured distribution are in `docs/album-database.md`):
///
/// - **Artist variation is explicitly not a signal.** Only `Album`,
///   `AlbumArtist` and year are consulted. A compilation with one
///   `AlbumArtist` and many track `Artist` values is `no_mbid_consistent`,
///   not varied.
/// - **Year is the first four characters of `Date`, and `'0000'` is filtered
///   out before counting distinct years** — tracks carrying a mix of a real
///   year and `'0000'` have one year, not two.
pub fn derive_tag_state(songs: &[SongTags]) -> TagState {
    assert!(
        !songs.is_empty(),
        "derive_tag_state requires at least one song"
    );

    let with_release_id: Vec<&str> = songs
        .iter()
        .filter_map(|s| present(&s.mb_release_id))
        .collect();

    if with_release_id.len() == songs.len() {
        let distinct: BTreeSet<&str> = with_release_id.into_iter().collect();
        return if distinct.len() == 1 {
            TagState::Clean
        } else {
            TagState::MixedMbid
        };
    }

    if !with_release_id.is_empty() {
        return TagState::PartialMbid;
    }

    let album_missing = songs.iter().any(|s| present(&s.album).is_none());
    let album_artist_missing = songs.iter().any(|s| present(&s.album_artist).is_none());
    if album_missing || album_artist_missing {
        return TagState::NoMbidIncomplete;
    }

    let distinct_albums: BTreeSet<&str> = songs.iter().filter_map(|s| present(&s.album)).collect();
    let distinct_years: BTreeSet<&str> = songs
        .iter()
        .filter_map(|s| present(&s.date))
        .map(year_of)
        .filter(|y| *y != "0000")
        .collect();

    if distinct_albums.len() > 1 || distinct_years.len() > 1 {
        TagState::NoMbidVaried
    } else {
        TagState::NoMbidConsistent
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::song_tags::song;
    use pretty_assertions::assert_eq;

    fn with_release_id(path: &str, id: &str) -> SongTags {
        SongTags {
            mb_release_id: Some(id.to_string()),
            ..song(path)
        }
    }

    #[test]
    fn clean_when_every_track_shares_one_release_id() {
        let songs = vec![
            with_release_id("t1", "abc-123"),
            with_release_id("t2", "abc-123"),
        ];
        assert_eq!(derive_tag_state(&songs), TagState::Clean);
    }

    #[test]
    fn mixed_mbid_when_every_track_has_an_id_but_they_differ() {
        let songs = vec![
            with_release_id("t1", "abc-123"),
            with_release_id("t2", "def-456"),
        ];
        assert_eq!(derive_tag_state(&songs), TagState::MixedMbid);
    }

    #[test]
    fn partial_mbid_when_some_tracks_lack_an_id() {
        let songs = vec![with_release_id("t1", "abc-123"), song("t2")];
        assert_eq!(derive_tag_state(&songs), TagState::PartialMbid);
    }

    #[test]
    fn no_mbid_consistent_when_album_and_artist_and_year_agree() {
        let songs = vec![
            SongTags {
                album: Some("Greatest Hits".to_string()),
                album_artist: Some("Various Artists".to_string()),
                artist: Some("Artist One".to_string()),
                date: Some("1999".to_string()),
                ..song("t1")
            },
            SongTags {
                album: Some("Greatest Hits".to_string()),
                album_artist: Some("Various Artists".to_string()),
                artist: Some("Artist Two".to_string()),
                date: Some("1999".to_string()),
                ..song("t2")
            },
        ];
        assert_eq!(derive_tag_state(&songs), TagState::NoMbidConsistent);
    }

    /// The trap this rule exists for: many distinct track `Artist` values
    /// under one `AlbumArtist` and one year is a normal compilation, not
    /// "varied". Only `Album`/`AlbumArtist`/year variation counts.
    #[test]
    fn many_artists_with_single_album_artist_and_year_is_consistent_not_varied() {
        let songs: Vec<SongTags> = (0..19)
            .map(|i| SongTags {
                album: Some("Songs for Becky".to_string()),
                album_artist: Some("Various Artists".to_string()),
                artist: Some(format!("Artist {i}")),
                date: Some("2003".to_string()),
                ..song(&format!("t{i}"))
            })
            .collect();
        assert_eq!(derive_tag_state(&songs), TagState::NoMbidConsistent);
    }

    #[test]
    fn no_mbid_varied_when_album_title_varies() {
        let songs = vec![
            SongTags {
                album: Some("Title A".to_string()),
                album_artist: Some("Various Artists".to_string()),
                date: Some("2003".to_string()),
                ..song("t1")
            },
            SongTags {
                album: Some("Title B".to_string()),
                album_artist: Some("Various Artists".to_string()),
                date: Some("2003".to_string()),
                ..song("t2")
            },
        ];
        assert_eq!(derive_tag_state(&songs), TagState::NoMbidVaried);
    }

    #[test]
    fn no_mbid_varied_when_year_varies() {
        let songs = vec![
            SongTags {
                album: Some("Compilation".to_string()),
                album_artist: Some("Various Artists".to_string()),
                date: Some("1787".to_string()),
                ..song("t1")
            },
            SongTags {
                album: Some("Compilation".to_string()),
                album_artist: Some("Various Artists".to_string()),
                date: Some("2003".to_string()),
                ..song("t2")
            },
        ];
        assert_eq!(derive_tag_state(&songs), TagState::NoMbidVaried);
    }

    /// Junk year `'0000'` mixed with a real year is not a variation — it's
    /// filtered out entirely, leaving one distinct year.
    #[test]
    fn junk_year_mixed_with_real_year_is_not_a_variation() {
        let songs = vec![
            SongTags {
                album: Some("Eine Kleine Nachtmusik".to_string()),
                album_artist: Some("Mozart".to_string()),
                date: Some("0000".to_string()),
                ..song("t1")
            },
            SongTags {
                album: Some("Eine Kleine Nachtmusik".to_string()),
                album_artist: Some("Mozart".to_string()),
                date: Some("1787".to_string()),
                ..song("t2")
            },
        ];
        assert_eq!(derive_tag_state(&songs), TagState::NoMbidConsistent);
    }

    #[test]
    fn year_of_handles_multibyte_dates_without_panicking() {
        assert_eq!(year_of("2020-01-01"), "2020");
        assert_eq!(year_of("199"), "199");
        assert_eq!(year_of(""), "");
        // A multi-byte character straddling byte index 4 — byte slicing panics
        // here, character slicing doesn't.
        assert_eq!(year_of("19\u{2014}9"), "19\u{2014}9");
        assert_eq!(year_of("\u{2014}\u{2014}\u{2014}\u{2014}\u{2014}"), "————");
    }

    #[test]
    fn no_mbid_incomplete_when_album_missing() {
        let songs = vec![SongTags {
            album_artist: Some("Madness".to_string()),
            ..song("t1")
        }];
        assert_eq!(derive_tag_state(&songs), TagState::NoMbidIncomplete);
    }

    #[test]
    fn no_mbid_incomplete_when_album_artist_missing() {
        let songs = vec![SongTags {
            album: Some("Neverwhere".to_string()),
            ..song("t1")
        }];
        assert_eq!(derive_tag_state(&songs), TagState::NoMbidIncomplete);
    }
}
