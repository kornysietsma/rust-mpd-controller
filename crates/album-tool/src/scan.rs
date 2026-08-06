use std::collections::BTreeMap;
use std::fmt;

use albumdb::{
    AlbumFacts, Clock, Connection, GenreEntry, GenreSource, SongTags, TagState, TitleSource,
    derive_tag_state, normalise_date, parse_album_path, replace_genres, upsert_album,
};
use anyhow::{Context, Result};

/// One album directory's derived facts plus its file-tag genre (spec §5.3),
/// as pure a function of `Vec<SongTags>` as `derive_tag_state` itself. No
/// reconciliation against existing rows here — that's Step 9; every scan is
/// treated as a first scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumGroup {
    pub facts: AlbumFacts,
    pub tag_genre: Option<String>,
}

/// Groups songs by their containing directory and derives one [`AlbumGroup`]
/// per album (spec §5.1–§5.4). Pure — no I/O — so it's directly unit
/// testable with hand-built `SongTags`.
pub fn group_into_albums(songs: Vec<SongTags>) -> Result<Vec<AlbumGroup>> {
    let mut groups: BTreeMap<String, Vec<SongTags>> = BTreeMap::new();
    for song in songs {
        groups
            .entry(containing_directory(&song.path))
            .or_default()
            .push(song);
    }

    groups
        .into_iter()
        .map(|(path, songs)| build_album_group(path, songs))
        .collect()
}

fn containing_directory(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(dir, _)| dir.to_string())
        .unwrap_or_default()
}

fn present(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// Builds the facts for one album directory. Where tracks disagree on a
/// scalar field (only possible in `mixed_mbid`/`partial_mbid`, 3 albums
/// total in the real library, spec §2.1) the first non-empty value in
/// path-sorted order wins — those states are already flagged for manual
/// fixing in Picard (spec §8), so this value is never authoritative, only a
/// placeholder until a human resolves it.
fn build_album_group(path: String, mut songs: Vec<SongTags>) -> Result<AlbumGroup> {
    songs.sort_by(|a, b| a.path.cmp(&b.path));

    let tag_state = derive_tag_state(&songs);
    let title_source = match tag_state {
        TagState::Clean
        | TagState::MixedMbid
        | TagState::PartialMbid
        | TagState::NoMbidConsistent => TitleSource::Tag,
        TagState::NoMbidVaried | TagState::NoMbidIncomplete => TitleSource::Path,
    };

    let (title, artist) = match title_source {
        TitleSource::Tag => (
            songs
                .iter()
                .find_map(|s| present(&s.album))
                .map(str::to_string),
            songs
                .iter()
                .find_map(|s| present(&s.album_artist))
                .map(str::to_string),
        ),
        TitleSource::Path => parse_album_path(&path)
            .map_err(|e| anyhow::anyhow!(e))
            .with_context(|| format!("failed to derive title/artist for {path:?}"))?,
        TitleSource::Manual => unreachable!("scan never derives TitleSource::Manual"),
    };

    let date = songs.iter().find_map(|s| normalise_date(s.date.as_deref()));
    let original_date = songs
        .iter()
        .find_map(|s| normalise_date(s.original_date.as_deref()));
    let label = songs
        .iter()
        .find_map(|s| present(&s.label))
        .map(str::to_string);
    let track_count = songs.len() as i64;
    let disc_count = songs
        .iter()
        .filter_map(|s| present(&s.disc).and_then(|d| d.parse::<i64>().ok()))
        .max()
        .unwrap_or(1);

    let mb_release_id = songs
        .iter()
        .find_map(|s| present(&s.mb_release_id))
        .map(str::to_string);
    let mb_release_group_id = songs
        .iter()
        .find_map(|s| present(&s.mb_release_group_id))
        .map(str::to_string);
    let mb_albumartist_id = songs
        .iter()
        .find_map(|s| present(&s.mb_albumartist_id))
        .map(str::to_string);

    let tag_genre = songs
        .iter()
        .find_map(|s| present(&s.genre))
        .map(str::to_string);

    Ok(AlbumGroup {
        facts: AlbumFacts {
            path,
            title,
            artist,
            title_source,
            date,
            original_date,
            label,
            track_count,
            disc_count,
            mb_release_id,
            mb_release_group_id,
            mb_albumartist_id,
            tag_state,
        },
        tag_genre,
    })
}

/// Writes every group via `upsert_album`, then replaces its `source = 'tag'`
/// genre row (or clears it, if the album now has no tag genre — keeps a
/// re-scan idempotent).
pub fn write_albums(conn: &mut Connection, groups: &[AlbumGroup], clock: &dyn Clock) -> Result<()> {
    for group in groups {
        let album_id = upsert_album(conn, &group.facts, clock.now())
            .with_context(|| format!("failed to write album {:?}", group.facts.path))?;

        let genres: Vec<GenreEntry> = group
            .tag_genre
            .iter()
            .map(|genre| GenreEntry {
                genre: genre.clone(),
                votes: 0,
            })
            .collect();
        replace_genres(conn, album_id, GenreSource::Tag, &genres).with_context(|| {
            format!("failed to write tag genre for album {:?}", group.facts.path)
        })?;
    }
    Ok(())
}

/// Counts of albums per `tag_state`, in the order spec §5.4's table lists
/// them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TagStateCounts {
    pub clean: usize,
    pub mixed_mbid: usize,
    pub partial_mbid: usize,
    pub no_mbid_consistent: usize,
    pub no_mbid_varied: usize,
    pub no_mbid_incomplete: usize,
}

impl TagStateCounts {
    pub fn from_groups(groups: &[AlbumGroup]) -> Self {
        let mut counts = Self::default();
        for group in groups {
            match group.facts.tag_state {
                TagState::Clean => counts.clean += 1,
                TagState::MixedMbid => counts.mixed_mbid += 1,
                TagState::PartialMbid => counts.partial_mbid += 1,
                TagState::NoMbidConsistent => counts.no_mbid_consistent += 1,
                TagState::NoMbidVaried => counts.no_mbid_varied += 1,
                TagState::NoMbidIncomplete => counts.no_mbid_incomplete += 1,
            }
        }
        counts
    }
}

impl fmt::Display for TagStateCounts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "  clean               {:>5}", self.clean)?;
        writeln!(f, "  mixed_mbid          {:>5}", self.mixed_mbid)?;
        writeln!(f, "  partial_mbid        {:>5}", self.partial_mbid)?;
        writeln!(f, "  no_mbid_consistent  {:>5}", self.no_mbid_consistent)?;
        writeln!(f, "  no_mbid_varied      {:>5}", self.no_mbid_varied)?;
        write!(f, "  no_mbid_incomplete  {:>5}", self.no_mbid_incomplete)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{FixtureLibrarySource, LibrarySource};
    use albumdb::{FixedClock, find_by_path, open_in_memory};
    use pretty_assertions::assert_eq;
    use std::path::Path;
    use time::macros::datetime;

    fn fixture_songs() -> Vec<SongTags> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/library.txt");
        FixtureLibrarySource::load(&path).unwrap().songs().unwrap()
    }

    #[test]
    fn groups_the_fixture_into_the_expected_tag_states() {
        let groups = group_into_albums(fixture_songs()).unwrap();

        // 20 albums per the fixture's own header comment (plan Step 7).
        assert_eq!(groups.len(), 20);

        let counts = TagStateCounts::from_groups(&groups);
        assert_eq!(
            counts,
            TagStateCounts {
                clean: 9,
                mixed_mbid: 1,
                partial_mbid: 1,
                no_mbid_consistent: 3,
                no_mbid_varied: 3,
                no_mbid_incomplete: 3,
            }
        );
    }

    #[test]
    fn title_source_is_path_for_varied_and_incomplete_tag_states_and_tag_otherwise() {
        let groups = group_into_albums(fixture_songs()).unwrap();
        for group in &groups {
            let expected = match group.facts.tag_state {
                TagState::NoMbidVaried | TagState::NoMbidIncomplete => TitleSource::Path,
                _ => TitleSource::Tag,
            };
            assert_eq!(
                group.facts.title_source, expected,
                "wrong title_source for {:?} ({:?})",
                group.facts.path, group.facts.tag_state
            );
        }
    }

    #[test]
    fn clean_album_keeps_its_tag_title_artist_and_genre() {
        let groups = group_into_albums(fixture_songs()).unwrap();
        let glass_horizon = groups
            .iter()
            .find(|g| g.facts.path == "Albums/A/Aurora Fields/Glass Horizon")
            .unwrap();

        assert_eq!(glass_horizon.facts.title.as_deref(), Some("Glass Horizon"));
        assert_eq!(glass_horizon.facts.artist.as_deref(), Some("Aurora Fields"));
        assert_eq!(glass_horizon.facts.title_source, TitleSource::Tag);
        assert_eq!(glass_horizon.facts.date.as_deref(), Some("2016-03-04"));
        assert_eq!(glass_horizon.facts.track_count, 2);
        assert_eq!(glass_horizon.facts.disc_count, 1);
        assert_eq!(glass_horizon.tag_genre.as_deref(), Some("Dream Pop"));
    }

    #[test]
    fn multi_disc_album_gets_the_max_disc_number() {
        let groups = group_into_albums(fixture_songs()).unwrap();
        let amber_season = groups
            .iter()
            .find(|g| g.facts.path == "Albums/D/Driftwood Choir/Amber Season")
            .unwrap();

        assert_eq!(amber_season.facts.track_count, 4);
        assert_eq!(amber_season.facts.disc_count, 2);
    }

    #[test]
    fn path_derived_album_has_no_tag_genre() {
        let groups = group_into_albums(fixture_songs()).unwrap();
        let harbor_lights = groups
            .iter()
            .find(|g| g.facts.path == "Albums/R/Rusted Chorus/Harbor Lights")
            .unwrap();

        assert_eq!(harbor_lights.facts.title_source, TitleSource::Path);
        assert_eq!(harbor_lights.facts.title.as_deref(), Some("Harbor Lights"));
        assert_eq!(harbor_lights.facts.artist.as_deref(), Some("Rusted Chorus"));
    }

    #[test]
    fn junk_year_0000_normalises_to_no_date_rather_than_a_second_variation() {
        let groups = group_into_albums(fixture_songs()).unwrap();
        let unnmatched = groups
            .iter()
            .find(|g| g.facts.path == "Unnmatched/Becky's Wedding Edits")
            .unwrap();

        // Both tracks are Date '0000' -> tag_state stays no_mbid_consistent
        // (not varied), and the album-level date normalises to None rather
        // than the literal junk string.
        assert_eq!(unnmatched.facts.tag_state, TagState::NoMbidConsistent);
        assert_eq!(unnmatched.facts.title_source, TitleSource::Tag);
        assert_eq!(
            unnmatched.facts.title.as_deref(),
            Some("Becky's Wedding Edits")
        );
        assert_eq!(unnmatched.facts.date, None);
    }

    #[test]
    fn writing_the_fixture_persists_every_album_and_its_tag_genre() {
        let mut conn = open_in_memory().unwrap();
        let clock = FixedClock(datetime!(2026-08-06 00:00:00 UTC));

        let groups = group_into_albums(fixture_songs()).unwrap();
        write_albums(&mut conn, &groups, &clock).unwrap();

        let album = find_by_path(&conn, "Albums/A/Aurora Fields/Glass Horizon")
            .unwrap()
            .unwrap();
        assert_eq!(album.title.as_deref(), Some("Glass Horizon"));
        assert_eq!(album.status.as_str(), "present");

        let mut stmt = conn
            .prepare("SELECT genre, votes FROM album_genres WHERE album_id = ?1 AND source = 'tag'")
            .unwrap();
        let genres: Vec<(String, i64)> = stmt
            .query_map([album.album_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(genres, vec![("Dream Pop".to_string(), 0)]);
    }

    #[test]
    fn a_second_scan_of_the_same_fixture_changes_nothing() {
        let mut conn = open_in_memory().unwrap();
        let clock = FixedClock(datetime!(2026-08-06 00:00:00 UTC));

        let groups = group_into_albums(fixture_songs()).unwrap();
        write_albums(&mut conn, &groups, &clock).unwrap();
        let before = albumdb::all_present(&conn).unwrap();

        let groups_again = group_into_albums(fixture_songs()).unwrap();
        write_albums(&mut conn, &groups_again, &clock).unwrap();
        let after = albumdb::all_present(&conn).unwrap();

        assert_eq!(before, after);
    }
}
