use std::collections::{BTreeMap, HashSet};
use std::fmt;

use albumdb::{
    AlbumFacts, Clock, Connection, GenreEntry, GenreSource, SongTags, TagState, TitleSource,
    all_present, derive_tag_state, find_by_path, mark_missing, normalise_date, parse_album_path,
    replace_genres, upsert_album,
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

/// Which paths a scan would add, update, or find missing (spec §5.2, §5.5):
/// `added` are paths with no existing row at all, `updated` are paths that
/// already had a row (present or missing) and were seen again in MPD, and
/// `newly_missing` are paths that were `status = 'present'` before this scan
/// but are absent from it. All three are path-sorted for stable output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanReport {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub newly_missing: Vec<String>,
}

impl fmt::Display for ScanReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "  added {} / updated {} / newly missing {}",
            self.added.len(),
            self.updated.len(),
            self.newly_missing.len()
        )?;
        if !self.added.is_empty() {
            writeln!(f, "  new:")?;
            for path in &self.added {
                writeln!(f, "    + {path}")?;
            }
        }
        if !self.newly_missing.is_empty() {
            writeln!(f, "  missing:")?;
            for path in &self.newly_missing {
                writeln!(f, "    - {path}")?;
            }
        }
        Ok(())
    }
}

/// Diffs `groups` against the albums currently `status = 'present'` in the
/// database, without writing anything — used by both `write_albums` (to know
/// what to mark missing) and `scan --dry-run` (which reports without
/// touching the database).
pub fn compute_report(conn: &Connection, groups: &[AlbumGroup]) -> Result<ScanReport> {
    let existing_present: HashSet<String> = all_present(conn)
        .context("failed to read existing present albums")?
        .into_iter()
        .map(|a| a.path)
        .collect();

    let mut added = Vec::new();
    let mut updated = Vec::new();
    for group in groups {
        let existed = find_by_path(conn, &group.facts.path)
            .with_context(|| format!("failed to look up album {:?}", group.facts.path))?
            .is_some();
        if existed {
            updated.push(group.facts.path.clone());
        } else {
            added.push(group.facts.path.clone());
        }
    }

    let scanned_paths: HashSet<&str> = groups.iter().map(|g| g.facts.path.as_str()).collect();
    let mut newly_missing: Vec<String> = existing_present
        .into_iter()
        .filter(|path| !scanned_paths.contains(path.as_str()))
        .collect();
    newly_missing.sort();

    Ok(ScanReport {
        added,
        updated,
        newly_missing,
    })
}

/// Writes every group via `upsert_album`, then replaces its `source = 'tag'`
/// genre row (or clears it, if the album now has no tag genre — keeps a
/// re-scan idempotent). Also reconciles the other direction (spec §5.2):
/// any album still `status = 'present'` from a previous scan but absent from
/// `groups` is marked `missing`, never deleted. Returns the diff computed by
/// [`compute_report`].
pub fn write_albums(
    conn: &mut Connection,
    groups: &[AlbumGroup],
    clock: &dyn Clock,
) -> Result<ScanReport> {
    let report = compute_report(conn, groups)?;

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

    for path in &report.newly_missing {
        let album = find_by_path(conn, path)
            .with_context(|| format!("failed to look up album {path:?}"))?
            .with_context(|| format!("album {path:?} vanished mid-scan"))?;
        mark_missing(conn, album.album_id)
            .with_context(|| format!("failed to mark album {path:?} missing"))?;
    }

    Ok(report)
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
        Self::from_tag_states(groups.iter().map(|g| g.facts.tag_state))
    }

    /// Shared with `status`, which counts `tag_state` straight off `Album`
    /// rows read back from the database rather than freshly scanned groups.
    pub fn from_tag_states(states: impl Iterator<Item = TagState>) -> Self {
        let mut counts = Self::default();
        for state in states {
            match state {
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
    use albumdb::{FixedClock, find_by_path, open_in_memory, set_rating};
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

    /// Drops one album out of the fixture entirely, simulating a directory
    /// that vanished from MPD (plan Step 9).
    fn without_glass_horizon() -> Vec<SongTags> {
        fixture_songs()
            .into_iter()
            .filter(|s| !s.path.starts_with("Albums/A/Aurora Fields/Glass Horizon/"))
            .collect()
    }

    #[test]
    fn removed_album_becomes_missing_with_rating_intact() {
        let mut conn = open_in_memory().unwrap();
        let clock1 = FixedClock(datetime!(2026-08-06 00:00:00 UTC));

        let groups = group_into_albums(fixture_songs()).unwrap();
        write_albums(&mut conn, &groups, &clock1).unwrap();

        let album = find_by_path(&conn, "Albums/A/Aurora Fields/Glass Horizon")
            .unwrap()
            .unwrap();
        set_rating(&conn, album.album_id, Some(5), clock1.0).unwrap();

        let clock2 = FixedClock(datetime!(2026-08-07 00:00:00 UTC));
        let groups_without = group_into_albums(without_glass_horizon()).unwrap();
        let report = write_albums(&mut conn, &groups_without, &clock2).unwrap();

        assert_eq!(
            report.newly_missing,
            vec!["Albums/A/Aurora Fields/Glass Horizon".to_string()]
        );
        assert!(report.added.is_empty());

        let missing = find_by_path(&conn, "Albums/A/Aurora Fields/Glass Horizon")
            .unwrap()
            .unwrap();
        assert_eq!(missing.status.as_str(), "missing");
        assert_eq!(missing.rating, Some(5));
        // `last_seen` records "last seen present" — going missing must not
        // touch it (spec §5.2).
        assert_eq!(missing.last_seen, "2026-08-06T00:00:00Z");
    }

    #[test]
    fn album_reappearing_after_missing_returns_to_present_with_rating_intact() {
        let mut conn = open_in_memory().unwrap();
        let clock1 = FixedClock(datetime!(2026-08-06 00:00:00 UTC));

        let all_songs = fixture_songs();
        write_albums(
            &mut conn,
            &group_into_albums(all_songs.clone()).unwrap(),
            &clock1,
        )
        .unwrap();

        let album = find_by_path(&conn, "Albums/A/Aurora Fields/Glass Horizon")
            .unwrap()
            .unwrap();
        set_rating(&conn, album.album_id, Some(5), clock1.0).unwrap();

        let clock2 = FixedClock(datetime!(2026-08-07 00:00:00 UTC));
        write_albums(
            &mut conn,
            &group_into_albums(without_glass_horizon()).unwrap(),
            &clock2,
        )
        .unwrap();
        assert_eq!(
            find_by_path(&conn, "Albums/A/Aurora Fields/Glass Horizon")
                .unwrap()
                .unwrap()
                .status
                .as_str(),
            "missing"
        );

        let clock3 = FixedClock(datetime!(2026-08-08 00:00:00 UTC));
        let report =
            write_albums(&mut conn, &group_into_albums(all_songs).unwrap(), &clock3).unwrap();

        assert!(report.newly_missing.is_empty());
        assert!(
            report
                .updated
                .contains(&"Albums/A/Aurora Fields/Glass Horizon".to_string())
        );

        let reappeared = find_by_path(&conn, "Albums/A/Aurora Fields/Glass Horizon")
            .unwrap()
            .unwrap();
        assert_eq!(reappeared.status.as_str(), "present");
        assert_eq!(reappeared.rating, Some(5));
        assert_eq!(reappeared.last_seen, "2026-08-08T00:00:00Z");
    }

    #[test]
    fn renamed_directory_shows_as_a_new_and_a_missing_row_with_rating_intact() {
        let mut conn = open_in_memory().unwrap();
        let clock1 = FixedClock(datetime!(2026-08-06 00:00:00 UTC));

        let groups = group_into_albums(fixture_songs()).unwrap();
        write_albums(&mut conn, &groups, &clock1).unwrap();

        let album = find_by_path(&conn, "Albums/A/Aurora Fields/Glass Horizon")
            .unwrap()
            .unwrap();
        set_rating(&conn, album.album_id, Some(4), clock1.0).unwrap();

        let renamed: Vec<SongTags> = fixture_songs()
            .into_iter()
            .map(|mut s| {
                if s.path.starts_with("Albums/A/Aurora Fields/Glass Horizon/") {
                    s.path = s.path.replacen(
                        "Albums/A/Aurora Fields/Glass Horizon/",
                        "Albums/A/Aurora Fields/Glass Horizon (Remastered)/",
                        1,
                    );
                }
                s
            })
            .collect();

        let clock2 = FixedClock(datetime!(2026-08-07 00:00:00 UTC));
        let report =
            write_albums(&mut conn, &group_into_albums(renamed).unwrap(), &clock2).unwrap();

        // Deliberately not auto-matched (spec §5.2, §9): a rename produces
        // one new unrated row and one missing row that keeps its rating,
        // visible side by side in the report rather than silently merged.
        assert_eq!(
            report.added,
            vec!["Albums/A/Aurora Fields/Glass Horizon (Remastered)".to_string()]
        );
        assert_eq!(
            report.newly_missing,
            vec!["Albums/A/Aurora Fields/Glass Horizon".to_string()]
        );

        let old = find_by_path(&conn, "Albums/A/Aurora Fields/Glass Horizon")
            .unwrap()
            .unwrap();
        assert_eq!(old.status.as_str(), "missing");
        assert_eq!(old.rating, Some(4));

        let new = find_by_path(&conn, "Albums/A/Aurora Fields/Glass Horizon (Remastered)")
            .unwrap()
            .unwrap();
        assert_eq!(new.status.as_str(), "present");
        assert_eq!(new.rating, None);
    }
}
