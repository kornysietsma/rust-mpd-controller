use albumdb::SongTags;

/// Builds a `find`/`lsinfo`-style `(base '<path>')` filter clause, escaping
/// `path` for MPD's filter-expression single-quoted string syntax.
///
/// This is the *inner* level of MPD's two-level quoting: the escaped
/// clause is then handed to `RawCommand::argument`, which applies the
/// *outer* protocol-level escaping (backslash-escaping `'`, `"`, `\`, and
/// wrapping the whole argument in double quotes because album paths contain
/// spaces) automatically — see `mpd_protocol::command::escape_argument`.
/// Nothing here needs to duplicate that outer layer.
pub fn base_filter(path: &str) -> String {
    format!("(base '{}')", escape_filter_value(path))
}

fn escape_filter_value(value: &str) -> String {
    // Order matters: escape backslashes first, so the backslash inserted for
    // an apostrophe isn't itself re-escaped.
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

/// The result of parsing an MPD `lsinfo`/`listfiles`-style response: the
/// subdirectory names present, plus any songs listed directly (files that
/// are immediate children of the queried directory, not nested in any of
/// the listed subdirectories).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirListing {
    pub directories: Vec<String>,
    pub songs: Vec<SongTags>,
}

/// Parses a flat stream of MPD response `(key, value)` pairs into a
/// [`DirListing`]. Shared by every command this crate hand-rolls (`find`,
/// `lsinfo`, `listfiles`) since they all use the same record-boundary
/// convention: a `file`/`directory`/`playlist` field starts a new record,
/// and any preceding song is complete at that point. `find` responses never
/// contain `directory`/`playlist` fields (recursion means no directory
/// markers are ever returned), so `directories` is simply empty for those —
/// no special-casing needed at the call site.
///
/// Unrecognized fields (`Last-Modified`, `Added`, `Format`, `Time`,
/// `duration`, `*Sort` fields, and any tag this project doesn't track) are
/// silently ignored via [`apply_tag`]'s catch-all, mirroring how
/// `mpd_client`'s own (private) `SongBuilder` treats them.
///
/// **`Last-Modified` is not a record boundary** — MPD sends it as the field
/// immediately after every `file:` line, so treating it as one (an earlier
/// version of this function did, alongside `playlist`) silently dropped
/// every tag that followed it: confirmed live, every song came back with
/// `path` set and every other field `None`.
pub fn parse_directory_listing<I>(fields: I) -> DirListing
where
    I: IntoIterator<Item = (String, String)>,
{
    let mut listing = DirListing::default();
    let mut current: Option<SongTags> = None;

    for (key, value) in fields {
        match key.as_str() {
            "file" => {
                if let Some(song) = current.take() {
                    listing.songs.push(song);
                }
                current = Some(SongTags {
                    path: value,
                    ..Default::default()
                });
            }
            "directory" => {
                if let Some(song) = current.take() {
                    listing.songs.push(song);
                }
                listing.directories.push(value);
            }
            "playlist" => {
                if let Some(song) = current.take() {
                    listing.songs.push(song);
                }
            }
            _ => {
                if let Some(song) = current.as_mut() {
                    apply_tag(song, &key, value);
                }
            }
        }
    }

    if let Some(song) = current.take() {
        listing.songs.push(song);
    }

    listing
}

fn apply_tag(song: &mut SongTags, key: &str, value: String) {
    match key {
        "Album" => song.album = Some(value),
        "AlbumArtist" => song.album_artist = Some(value),
        "Artist" => song.artist = Some(value),
        "Title" => song.title = Some(value),
        "Track" => song.track = Some(value),
        "Date" => song.date = Some(value),
        "OriginalDate" => song.original_date = Some(value),
        "Genre" => song.genre = Some(value),
        "Label" => song.label = Some(value),
        "Disc" => song.disc = Some(value),
        "MUSICBRAINZ_ALBUMID" => song.mb_release_id = Some(value),
        "MUSICBRAINZ_RELEASEGROUPID" => song.mb_release_group_id = Some(value),
        "MUSICBRAINZ_RELEASETRACKID" => song.mb_track_id = Some(value),
        "MUSICBRAINZ_ALBUMARTISTID" => song.mb_albumartist_id = Some(value),
        // Anything else (duration, format, other MusicBrainz ids, etc.) is
        // not part of the album database and is intentionally dropped.
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn base_filter_escapes_apostrophe() {
        // The real path shape that broke the first quoting attempt.
        assert_eq!(
            base_filter("A Wake-Up Call for the '90s"),
            r"(base 'A Wake-Up Call for the \'90s')"
        );
    }

    #[test]
    fn base_filter_escapes_backslash() {
        assert_eq!(base_filter(r"foo\bar"), r"(base 'foo\\bar')");
    }

    #[test]
    fn base_filter_plain_path_unchanged() {
        assert_eq!(base_filter("Albums/A"), "(base 'Albums/A')");
    }

    #[test]
    fn base_filter_backslash_before_apostrophe_not_double_escaped() {
        // Backslashes are escaped first; if apostrophe-escaping ran first
        // and re-escaped the backslash it just inserted, this would produce
        // an extra backslash.
        assert_eq!(base_filter(r"it\'s"), r"(base 'it\\\'s')");
    }

    #[test]
    fn parses_a_single_song() {
        let listing = parse_directory_listing(pairs(&[
            ("file", "Albums/A/Artist/Album/01 - Song.mp3"),
            ("Album", "Album"),
            ("AlbumArtist", "Artist"),
            ("Artist", "Artist"),
            ("Title", "Song"),
            ("Track", "1"),
            ("Date", "2020"),
            ("OriginalDate", "2019"),
            ("Genre", "Rock"),
            ("Label", "Some Label"),
            ("Disc", "1"),
            ("MUSICBRAINZ_ALBUMID", "release-id"),
            ("MUSICBRAINZ_RELEASEGROUPID", "release-group-id"),
            ("MUSICBRAINZ_RELEASETRACKID", "track-id"),
            ("MUSICBRAINZ_ALBUMARTISTID", "albumartist-id"),
        ]));

        assert_eq!(listing.directories, Vec::<String>::new());
        assert_eq!(
            listing.songs,
            vec![SongTags {
                path: "Albums/A/Artist/Album/01 - Song.mp3".to_string(),
                album: Some("Album".to_string()),
                album_artist: Some("Artist".to_string()),
                artist: Some("Artist".to_string()),
                title: Some("Song".to_string()),
                track: Some("1".to_string()),
                date: Some("2020".to_string()),
                original_date: Some("2019".to_string()),
                genre: Some("Rock".to_string()),
                label: Some("Some Label".to_string()),
                disc: Some("1".to_string()),
                mb_release_id: Some("release-id".to_string()),
                mb_release_group_id: Some("release-group-id".to_string()),
                mb_track_id: Some("track-id".to_string()),
                mb_albumartist_id: Some("albumartist-id".to_string()),
            }]
        );
    }

    #[test]
    fn parses_multiple_songs_split_on_file() {
        let listing = parse_directory_listing(pairs(&[
            ("file", "a.mp3"),
            ("Title", "A"),
            ("file", "b.mp3"),
            ("Title", "B"),
        ]));

        assert_eq!(listing.songs.len(), 2);
        assert_eq!(listing.songs[0].path, "a.mp3");
        assert_eq!(listing.songs[0].title, Some("A".to_string()));
        assert_eq!(listing.songs[1].path, "b.mp3");
        assert_eq!(listing.songs[1].title, Some("B".to_string()));
    }

    /// Regression test for a real bug: `Last-Modified` immediately follows
    /// `file:` on every real MPD response (confirmed live), and an earlier
    /// version of this parser treated it as a record boundary — silently
    /// dropping every tag field that came after it, on every song, in the
    /// real library. `Album` here is deliberately placed *after*
    /// `Last-Modified`, matching MPD's actual field order.
    #[test]
    fn last_modified_does_not_terminate_the_current_song() {
        let listing = parse_directory_listing(pairs(&[
            ("file", "Albums/A/Artist/Album/01 - Song.mp3"),
            ("Last-Modified", "2021-02-02T11:56:54Z"),
            ("Added", "2025-09-07T10:09:12Z"),
            ("Format", "44100:16:2"),
            ("Artist", "Artist"),
            ("AlbumArtist", "Artist"),
            ("Album", "Album"),
            ("MUSICBRAINZ_RELEASEGROUPID", "release-group-id"),
        ]));

        assert_eq!(listing.songs.len(), 1);
        let song = &listing.songs[0];
        assert_eq!(song.artist.as_deref(), Some("Artist"));
        assert_eq!(song.album_artist.as_deref(), Some("Artist"));
        assert_eq!(song.album.as_deref(), Some("Album"));
        assert_eq!(
            song.mb_release_group_id.as_deref(),
            Some("release-group-id")
        );
    }

    #[test]
    fn directory_entries_are_collected_and_not_treated_as_songs() {
        let listing = parse_directory_listing(pairs(&[
            ("directory", "Albums/A"),
            ("directory", "Albums/B"),
        ]));

        assert_eq!(listing.directories, vec!["Albums/A", "Albums/B"]);
        assert!(listing.songs.is_empty());
    }

    #[test]
    fn a_directory_entry_completes_a_pending_song() {
        // lsinfo's response can mix loose files and subdirectories: a
        // `directory` field ends whatever song was being built, exactly
        // like a following `file` field would.
        let listing = parse_directory_listing(pairs(&[
            ("file", "Compilations/loose.mp3"),
            ("Title", "Loose"),
            ("directory", "Compilations/Some Album"),
        ]));

        assert_eq!(listing.songs.len(), 1);
        assert_eq!(listing.songs[0].path, "Compilations/loose.mp3");
        assert_eq!(listing.directories, vec!["Compilations/Some Album"]);
    }

    #[test]
    fn unrecognized_fields_before_any_file_are_ignored() {
        let listing = parse_directory_listing(pairs(&[
            ("playlist", "some.m3u"),
            ("Last-Modified", "2020-01-01T00:00:00Z"),
            ("file", "a.mp3"),
        ]));

        assert_eq!(listing.songs.len(), 1);
        assert_eq!(listing.songs[0].path, "a.mp3");
    }

    #[test]
    fn empty_input_yields_empty_listing() {
        let listing = parse_directory_listing(pairs(&[]));
        assert_eq!(listing, DirListing::default());
    }
}
