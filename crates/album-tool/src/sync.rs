use std::collections::BTreeMap;
use std::fmt;

use albumdb::{Album, Clock, Connection, pending_sync_albums, set_sync_failed};
use anyhow::{Context, Result};

use crate::musicbrainz::MusicBrainzClient;

/// What one `sync` run did (spec §5.5-style report, plan Step 12).
///
/// **Successful fetches are not yet persisted** — Step 12 is transport
/// only. A row that fetches cleanly stays `mb_sync_state = 'pending'` until
/// Step 13 lands and writes genres + `ok` together, atomically, so a row is
/// never `ok` without the genre data that justifies it. `albums_fetched_ok`
/// exists purely so this run's own report is honest about what happened.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub release_groups_attempted: usize,
    pub albums_fetched_ok: usize,
    pub albums_failed: usize,
    pub failures: Vec<(String, String)>,
}

impl fmt::Display for SyncReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "  {} release group(s) requested: {} fetched ok (not yet persisted — Step 13), {} failed",
            self.release_groups_attempted, self.albums_fetched_ok, self.albums_failed
        )?;
        if !self.failures.is_empty() {
            writeln!(f, "  failed:")?;
            for (path, error) in &self.failures {
                writeln!(f, "    ! {path}: {error}")?;
            }
        }
        Ok(())
    }
}

/// Runs one `sync` pass (spec §7): selects eligible albums, deduplicates by
/// `mb_release_group_id` (spec §7.1 — 1,830 groups across 1,886 albums),
/// and fetches each group once via `client`, applying the outcome to every
/// album that shares it.
///
/// Every failure is committed as it happens — one `UPDATE` per album, not
/// one transaction for the whole run — so an interrupted run resumes
/// exactly where it stopped (spec §7.2) rather than losing progress already
/// made.
pub async fn run_sync<C: MusicBrainzClient>(
    conn: &Connection,
    client: &C,
    limit: Option<usize>,
    force: bool,
    clock: &dyn Clock,
) -> Result<SyncReport> {
    let candidates =
        pending_sync_albums(conn, force).context("failed to select albums pending sync")?;

    let mut by_group: BTreeMap<String, Vec<Album>> = BTreeMap::new();
    for album in candidates {
        let group_id = album
            .mb_release_group_id
            .clone()
            .expect("pending_sync_albums only returns rows with mb_release_group_id set");
        by_group.entry(group_id).or_default().push(album);
    }

    let mut groups: Vec<(String, Vec<Album>)> = by_group.into_iter().collect();
    if let Some(limit) = limit {
        groups.truncate(limit);
    }

    let mut report = SyncReport::default();
    for (mbid, albums) in groups {
        report.release_groups_attempted += 1;
        match client.release_group(&mbid).await {
            Ok(_data) => {
                report.albums_fetched_ok += albums.len();
            }
            Err(err) => {
                let reason = err.to_string();
                for album in &albums {
                    set_sync_failed(conn, album.album_id, &reason, clock.now()).with_context(
                        || format!("failed to record sync failure for {:?}", album.path),
                    )?;
                    report.failures.push((album.path.clone(), reason.clone()));
                }
                report.albums_failed += albums.len();
            }
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::musicbrainz::{
        FakeMusicBrainzClient, GenreVote, MusicBrainzError, ReleaseGroupData,
    };
    use albumdb::{
        AlbumFacts, FixedClock, MatchPolicy, TagState, TitleSource, open_in_memory,
        set_match_policy, upsert_album,
    };
    use pretty_assertions::assert_eq;
    use std::collections::HashMap;
    use time::macros::datetime;

    fn facts(path: &str, mb_release_group_id: Option<&str>) -> AlbumFacts {
        AlbumFacts {
            path: path.to_string(),
            title: Some("Some Album".to_string()),
            artist: Some("Some Artist".to_string()),
            title_source: TitleSource::Tag,
            date: Some("1999".to_string()),
            original_date: Some("1999".to_string()),
            label: None,
            track_count: 10,
            disc_count: 1,
            mb_release_id: Some("mb-release-1".to_string()),
            mb_release_group_id: mb_release_group_id.map(str::to_string),
            mb_albumartist_id: Some("mb-artist-1".to_string()),
            tag_state: TagState::Clean,
        }
    }

    fn ok_response() -> Result<ReleaseGroupData, MusicBrainzError> {
        Ok(ReleaseGroupData {
            genres: vec![GenreVote {
                genre: "dream pop".to_string(),
                votes: 5,
            }],
        })
    }

    #[tokio::test]
    async fn selection_honours_match_policy_never() {
        let conn = open_in_memory().unwrap();
        let now = datetime!(2026-08-05 00:00:00 UTC);
        let clock = FixedClock(now);

        let never_id =
            upsert_album(&conn, &facts("Albums/A/Artist/Never", Some("rg-1")), now).unwrap();
        set_match_policy(&conn, never_id, MatchPolicy::Never).unwrap();
        upsert_album(&conn, &facts("Albums/B/Artist/Eligible", Some("rg-2")), now).unwrap();

        let client =
            FakeMusicBrainzClient::new(HashMap::from([("rg-2".to_string(), vec![ok_response()])]));

        let report = run_sync(&conn, &client, None, false, &clock).await.unwrap();

        assert_eq!(report.release_groups_attempted, 1);
        assert_eq!(client.calls.lock().unwrap().as_slice(), ["rg-2"]);
    }

    #[tokio::test]
    async fn deduplicates_by_release_group_before_requesting() {
        let conn = open_in_memory().unwrap();
        let now = datetime!(2026-08-05 00:00:00 UTC);
        let clock = FixedClock(now);

        upsert_album(&conn, &facts("Albums/A/Artist/One", Some("rg-shared")), now).unwrap();
        upsert_album(&conn, &facts("Albums/B/Artist/Two", Some("rg-shared")), now).unwrap();

        let client = FakeMusicBrainzClient::new(HashMap::from([(
            "rg-shared".to_string(),
            vec![ok_response()],
        )]));

        let report = run_sync(&conn, &client, None, false, &clock).await.unwrap();

        assert_eq!(report.release_groups_attempted, 1);
        assert_eq!(report.albums_fetched_ok, 2);
        assert_eq!(client.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_success_is_not_persisted_yet_row_stays_pending() {
        let conn = open_in_memory().unwrap();
        let now = datetime!(2026-08-05 00:00:00 UTC);
        let clock = FixedClock(now);

        upsert_album(&conn, &facts("Albums/A/Artist/One", Some("rg-1")), now).unwrap();
        let client =
            FakeMusicBrainzClient::new(HashMap::from([("rg-1".to_string(), vec![ok_response()])]));

        run_sync(&conn, &client, None, false, &clock).await.unwrap();

        let album = albumdb::find_by_path(&conn, "Albums/A/Artist/One")
            .unwrap()
            .unwrap();
        assert_eq!(album.mb_sync_state.as_str(), "pending");
    }

    #[tokio::test]
    async fn a_404_is_recorded_as_failed_and_the_run_continues() {
        let conn = open_in_memory().unwrap();
        let now = datetime!(2026-08-05 00:00:00 UTC);
        let clock = FixedClock(now);

        upsert_album(&conn, &facts("Albums/A/Artist/Gone", Some("rg-404")), now).unwrap();
        upsert_album(&conn, &facts("Albums/B/Artist/Fine", Some("rg-ok")), now).unwrap();

        let client = FakeMusicBrainzClient::new(HashMap::from([
            ("rg-404".to_string(), vec![Err(MusicBrainzError::NotFound)]),
            ("rg-ok".to_string(), vec![ok_response()]),
        ]));

        let report = run_sync(&conn, &client, None, false, &clock).await.unwrap();

        assert_eq!(report.albums_failed, 1);
        assert_eq!(report.albums_fetched_ok, 1);

        let gone = albumdb::find_by_path(&conn, "Albums/A/Artist/Gone")
            .unwrap()
            .unwrap();
        assert_eq!(gone.mb_sync_state.as_str(), "failed");
        assert!(gone.mb_sync_error.unwrap().contains("not found"));
        assert_eq!(gone.mb_synced_at.as_deref(), Some("2026-08-05T00:00:00Z"));
    }

    #[tokio::test]
    async fn a_give_up_after_retries_is_recorded_as_failed() {
        let conn = open_in_memory().unwrap();
        let now = datetime!(2026-08-05 00:00:00 UTC);
        let clock = FixedClock(now);

        upsert_album(&conn, &facts("Albums/A/Artist/Flaky", Some("rg-1")), now).unwrap();
        let client = FakeMusicBrainzClient::new(HashMap::from([(
            "rg-1".to_string(),
            vec![Err(MusicBrainzError::Other(
                "gave up after 3 attempts".to_string(),
            ))],
        )]));

        let report = run_sync(&conn, &client, None, false, &clock).await.unwrap();
        assert_eq!(report.albums_failed, 1);

        let album = albumdb::find_by_path(&conn, "Albums/A/Artist/Flaky")
            .unwrap()
            .unwrap();
        assert_eq!(album.mb_sync_state.as_str(), "failed");
        assert_eq!(
            album.mb_sync_error.as_deref(),
            Some("gave up after 3 attempts")
        );
    }

    #[tokio::test]
    async fn limit_caps_the_number_of_release_groups_requested() {
        let conn = open_in_memory().unwrap();
        let now = datetime!(2026-08-05 00:00:00 UTC);
        let clock = FixedClock(now);

        upsert_album(&conn, &facts("Albums/A/Artist/One", Some("rg-1")), now).unwrap();
        upsert_album(&conn, &facts("Albums/B/Artist/Two", Some("rg-2")), now).unwrap();

        let client = FakeMusicBrainzClient::new(HashMap::from([
            ("rg-1".to_string(), vec![ok_response()]),
            ("rg-2".to_string(), vec![ok_response()]),
        ]));

        let report = run_sync(&conn, &client, Some(1), false, &clock)
            .await
            .unwrap();
        assert_eq!(report.release_groups_attempted, 1);
    }

    /// Resumability (spec §7.2): a failure already committed on a previous
    /// run is never re-requested by a plain re-run, since it's no longer
    /// `pending`. Simulates "interrupted mid-run" by running twice against
    /// the same database.
    #[tokio::test]
    async fn a_previously_failed_album_is_not_reattempted_without_force() {
        let conn = open_in_memory().unwrap();
        let now = datetime!(2026-08-05 00:00:00 UTC);
        let clock = FixedClock(now);

        upsert_album(&conn, &facts("Albums/A/Artist/One", Some("rg-1")), now).unwrap();
        let first_client = FakeMusicBrainzClient::new(HashMap::from([(
            "rg-1".to_string(),
            vec![Err(MusicBrainzError::NotFound)],
        )]));
        run_sync(&conn, &first_client, None, false, &clock)
            .await
            .unwrap();

        let second_client = FakeMusicBrainzClient::new(HashMap::new());
        let report = run_sync(&conn, &second_client, None, false, &clock)
            .await
            .unwrap();

        assert_eq!(report.release_groups_attempted, 0);
    }

    #[tokio::test]
    async fn force_reattempts_a_previously_failed_album() {
        let conn = open_in_memory().unwrap();
        let now = datetime!(2026-08-05 00:00:00 UTC);
        let clock = FixedClock(now);

        upsert_album(&conn, &facts("Albums/A/Artist/One", Some("rg-1")), now).unwrap();
        let first_client = FakeMusicBrainzClient::new(HashMap::from([(
            "rg-1".to_string(),
            vec![Err(MusicBrainzError::NotFound)],
        )]));
        run_sync(&conn, &first_client, None, false, &clock)
            .await
            .unwrap();

        let second_client =
            FakeMusicBrainzClient::new(HashMap::from([("rg-1".to_string(), vec![ok_response()])]));
        let report = run_sync(&conn, &second_client, None, true, &clock)
            .await
            .unwrap();

        assert_eq!(report.release_groups_attempted, 1);
        assert_eq!(report.albums_fetched_ok, 1);
    }
}
