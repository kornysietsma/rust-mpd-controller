use std::fmt;

use albumdb::{Album, AlbumStatus, Connection, SyncState, TagState};
use anyhow::{Context, Result};

use crate::scan::TagStateCounts;

/// Counts of present albums per `mb_sync_state`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncStateCounts {
    pub pending: usize,
    pub ok: usize,
    pub failed: usize,
    pub skipped: usize,
}

impl SyncStateCounts {
    fn from_albums<'a>(albums: impl Iterator<Item = &'a Album>) -> Self {
        let mut counts = Self::default();
        for album in albums {
            match album.mb_sync_state {
                SyncState::Pending => counts.pending += 1,
                SyncState::Ok => counts.ok += 1,
                SyncState::Failed => counts.failed += 1,
                SyncState::Skipped => counts.skipped += 1,
            }
        }
        counts
    }
}

impl fmt::Display for SyncStateCounts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "  pending  {:>5}", self.pending)?;
        writeln!(f, "  ok       {:>5}", self.ok)?;
        writeln!(f, "  failed   {:>5}", self.failed)?;
        write!(f, "  skipped  {:>5}", self.skipped)
    }
}

/// Read-only report over the current database state (spec §8). Every list is
/// path-sorted for stable output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusReport {
    pub total_present: usize,
    pub tag_state_counts: TagStateCounts,
    pub sync_state_counts: SyncStateCounts,
    /// Paths with `status = 'missing'`.
    pub missing: Vec<String>,
    /// `mixed_mbid` / `partial_mbid` albums — reported for manual fixing in
    /// Picard, never edited by the tool itself.
    pub needs_manual_fix: Vec<(String, TagState)>,
    /// `mb_sync_state = 'failed'` albums with their recorded error.
    pub sync_failures: Vec<(String, String)>,
    pub rated_count: usize,
    /// Paths with `needs_relisten = 1`.
    pub needs_relisten: Vec<String>,
}

pub fn compute_status_report(conn: &Connection) -> Result<StatusReport> {
    let albums = albumdb::all_albums(conn).context("failed to read albums for status report")?;

    let present: Vec<&Album> = albums
        .iter()
        .filter(|a| a.status == AlbumStatus::Present)
        .collect();

    let mut missing: Vec<String> = albums
        .iter()
        .filter(|a| a.status == AlbumStatus::Missing)
        .map(|a| a.path.clone())
        .collect();
    missing.sort();

    let mut needs_manual_fix: Vec<(String, TagState)> = present
        .iter()
        .filter(|a| matches!(a.tag_state, TagState::MixedMbid | TagState::PartialMbid))
        .map(|a| (a.path.clone(), a.tag_state))
        .collect();
    needs_manual_fix.sort_by(|a, b| a.0.cmp(&b.0));

    let mut sync_failures: Vec<(String, String)> = present
        .iter()
        .filter(|a| a.mb_sync_state == SyncState::Failed)
        .map(|a| {
            (
                a.path.clone(),
                a.mb_sync_error
                    .clone()
                    .unwrap_or_else(|| "(no error recorded)".to_string()),
            )
        })
        .collect();
    sync_failures.sort_by(|a, b| a.0.cmp(&b.0));

    let mut needs_relisten: Vec<String> = present
        .iter()
        .filter(|a| a.needs_relisten)
        .map(|a| a.path.clone())
        .collect();
    needs_relisten.sort();

    Ok(StatusReport {
        total_present: present.len(),
        tag_state_counts: TagStateCounts::from_tag_states(present.iter().map(|a| a.tag_state)),
        sync_state_counts: SyncStateCounts::from_albums(present.iter().copied()),
        missing,
        needs_manual_fix,
        sync_failures,
        rated_count: present.iter().filter(|a| a.rating.is_some()).count(),
        needs_relisten,
    })
}

impl fmt::Display for StatusReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "albums present: {}", self.total_present)?;
        writeln!(f)?;
        writeln!(f, "tag_state:")?;
        writeln!(f, "{}", self.tag_state_counts)?;
        writeln!(f)?;
        writeln!(f, "mb_sync_state:")?;
        writeln!(f, "{}", self.sync_state_counts)?;
        writeln!(f)?;
        writeln!(f, "rated: {}/{}", self.rated_count, self.total_present)?;
        writeln!(f, "needs_relisten: {}", self.needs_relisten.len())?;
        for path in &self.needs_relisten {
            writeln!(f, "    ~ {path}")?;
        }
        writeln!(f)?;
        writeln!(f, "missing: {}", self.missing.len())?;
        for path in &self.missing {
            writeln!(f, "    - {path}")?;
        }
        writeln!(f)?;
        writeln!(
            f,
            "needs manual fix in Picard (mixed_mbid / partial_mbid): {}",
            self.needs_manual_fix.len()
        )?;
        for (path, state) in &self.needs_manual_fix {
            writeln!(f, "    ! {path} ({})", state.as_str())?;
        }
        writeln!(f)?;
        write!(f, "sync failures: {}", self.sync_failures.len())?;
        for (path, error) in &self.sync_failures {
            write!(f, "\n    x {path}: {error}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use albumdb::{
        AlbumFacts, FixedClock, TitleSource, mark_missing, open_in_memory, set_needs_relisten,
        set_rating, upsert_album,
    };
    use pretty_assertions::assert_eq;
    use time::macros::datetime;

    fn facts(path: &str, tag_state: TagState) -> AlbumFacts {
        AlbumFacts {
            path: path.to_string(),
            title: Some("Some Title".to_string()),
            artist: Some("Some Artist".to_string()),
            title_source: TitleSource::Tag,
            date: None,
            original_date: None,
            label: None,
            track_count: 10,
            disc_count: 1,
            mb_release_id: None,
            mb_release_group_id: None,
            mb_albumartist_id: None,
            tag_state,
        }
    }

    #[test]
    fn report_covers_tag_states_sync_states_missing_ratings_and_relisten() {
        let conn = open_in_memory().unwrap();
        let clock = FixedClock(datetime!(2026-08-07 00:00:00 UTC));

        let clean = upsert_album(
            &conn,
            &facts("Albums/A/Artist/Clean", TagState::Clean),
            clock.0,
        )
        .unwrap();
        set_rating(&conn, clean, Some(5), clock.0).unwrap();

        let mixed = upsert_album(
            &conn,
            &facts("Albums/B/Artist/Mixed", TagState::MixedMbid),
            clock.0,
        )
        .unwrap();
        conn.execute(
            "UPDATE albums SET mb_sync_state = 'failed', mb_sync_error = 'boom' WHERE album_id = ?1",
            [mixed],
        )
        .unwrap();

        let relisten = upsert_album(
            &conn,
            &facts("Albums/C/Artist/Relisten", TagState::Clean),
            clock.0,
        )
        .unwrap();
        set_needs_relisten(&conn, relisten, true).unwrap();

        let gone = upsert_album(
            &conn,
            &facts("Albums/D/Artist/Gone", TagState::Clean),
            clock.0,
        )
        .unwrap();
        mark_missing(&conn, gone).unwrap();

        let report = compute_status_report(&conn).unwrap();

        assert_eq!(report.total_present, 3);
        assert_eq!(report.tag_state_counts.clean, 2);
        assert_eq!(report.tag_state_counts.mixed_mbid, 1);
        assert_eq!(report.sync_state_counts.failed, 1);
        assert_eq!(report.sync_state_counts.pending, 2);
        assert_eq!(report.missing, vec!["Albums/D/Artist/Gone".to_string()]);
        assert_eq!(
            report.needs_manual_fix,
            vec![("Albums/B/Artist/Mixed".to_string(), TagState::MixedMbid)]
        );
        assert_eq!(
            report.sync_failures,
            vec![("Albums/B/Artist/Mixed".to_string(), "boom".to_string())]
        );
        assert_eq!(report.rated_count, 1);
        assert_eq!(
            report.needs_relisten,
            vec!["Albums/C/Artist/Relisten".to_string()]
        );
    }

    #[test]
    fn a_missing_album_is_excluded_from_present_only_counts() {
        let conn = open_in_memory().unwrap();
        let clock = FixedClock(datetime!(2026-08-07 00:00:00 UTC));

        let gone = upsert_album(
            &conn,
            &facts("Albums/D/Artist/Gone", TagState::Clean),
            clock.0,
        )
        .unwrap();
        set_rating(&conn, gone, Some(4), clock.0).unwrap();
        mark_missing(&conn, gone).unwrap();

        let report = compute_status_report(&conn).unwrap();

        assert_eq!(report.total_present, 0);
        assert_eq!(report.rated_count, 0);
        assert_eq!(report.tag_state_counts, TagStateCounts::default());
        assert_eq!(report.missing, vec!["Albums/D/Artist/Gone".to_string()]);
    }
}
