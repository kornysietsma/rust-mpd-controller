use std::collections::BTreeMap;
use std::fmt;

use albumdb::{
    Album, Clock, Connection, GenreEntry, pending_sync_albums, set_sync_failed, set_sync_ok,
};
use anyhow::{Context, Result};

use crate::musicbrainz::MusicBrainzClient;

/// What one `sync` run did.
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
            "  {} release group(s) requested: {} fetched ok, {} failed",
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

/// Runs one `sync` pass: selects eligible albums, deduplicates by
/// `mb_release_group_id` (~1,830 groups across 1,886 albums in the real
/// library, so this is a real saving), and fetches each group once via
/// `client`, applying the outcome to every album that shares it.
///
/// Every outcome is committed as it happens — one `UPDATE` per album, not
/// one transaction for the whole run — so an interrupted run resumes exactly
/// where it stopped rather than losing progress already made.
pub async fn run_sync<C: MusicBrainzClient>(
    conn: &mut Connection,
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
            Ok(data) => {
                let genres: Vec<GenreEntry> = data
                    .genres
                    .iter()
                    .map(|g| GenreEntry {
                        genre: g.genre.clone(),
                        votes: g.votes,
                    })
                    .collect();
                for album in &albums {
                    set_sync_ok(
                        conn,
                        album.album_id,
                        &genres,
                        data.earliest_release_track_count,
                        clock.now(),
                    )
                    .with_context(|| {
                        format!("failed to persist sync result for {:?}", album.path)
                    })?;
                }
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
            earliest_release_track_count: None,
        })
    }

    #[tokio::test]
    async fn selection_honours_match_policy_never() {
        let mut conn = open_in_memory().unwrap();
        let now = datetime!(2026-08-05 00:00:00 UTC);
        let clock = FixedClock(now);

        let never_id =
            upsert_album(&conn, &facts("Albums/A/Artist/Never", Some("rg-1")), now).unwrap();
        set_match_policy(&conn, never_id, MatchPolicy::Never).unwrap();
        upsert_album(&conn, &facts("Albums/B/Artist/Eligible", Some("rg-2")), now).unwrap();

        let client =
            FakeMusicBrainzClient::new(HashMap::from([("rg-2".to_string(), vec![ok_response()])]));

        let report = run_sync(&mut conn, &client, None, false, &clock)
            .await
            .unwrap();

        assert_eq!(report.release_groups_attempted, 1);
        assert_eq!(client.calls.lock().unwrap().as_slice(), ["rg-2"]);
    }

    #[tokio::test]
    async fn deduplicates_by_release_group_before_requesting() {
        let mut conn = open_in_memory().unwrap();
        let now = datetime!(2026-08-05 00:00:00 UTC);
        let clock = FixedClock(now);

        upsert_album(&conn, &facts("Albums/A/Artist/One", Some("rg-shared")), now).unwrap();
        upsert_album(&conn, &facts("Albums/B/Artist/Two", Some("rg-shared")), now).unwrap();

        let client = FakeMusicBrainzClient::new(HashMap::from([(
            "rg-shared".to_string(),
            vec![ok_response()],
        )]));

        let report = run_sync(&mut conn, &client, None, false, &clock)
            .await
            .unwrap();

        assert_eq!(report.release_groups_attempted, 1);
        assert_eq!(report.albums_fetched_ok, 2);
        assert_eq!(client.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_success_is_persisted_genres_primary_genre_and_ok_state() {
        let mut conn = open_in_memory().unwrap();
        let now = datetime!(2026-08-05 00:00:00 UTC);
        let clock = FixedClock(now);

        upsert_album(&conn, &facts("Albums/A/Artist/One", Some("rg-1")), now).unwrap();
        let client = FakeMusicBrainzClient::new(HashMap::from([(
            "rg-1".to_string(),
            vec![Ok(ReleaseGroupData {
                genres: vec![
                    GenreVote {
                        genre: "hard rock".to_string(),
                        votes: 9,
                    },
                    GenreVote {
                        genre: "dark cabaret".to_string(),
                        votes: 5,
                    },
                ],
                earliest_release_track_count: Some(10),
            })],
        )]));

        run_sync(&mut conn, &client, None, false, &clock)
            .await
            .unwrap();

        let album = albumdb::find_by_path(&conn, "Albums/A/Artist/One")
            .unwrap()
            .unwrap();
        assert_eq!(album.mb_sync_state.as_str(), "ok");
        assert_eq!(album.primary_genre.as_deref(), Some("hard rock"));
        assert_eq!(album.mb_original_count, Some(10));
        assert_eq!(album.original_tracks.as_deref(), Some("1-10"));
        assert_eq!(album.original_tracks_src.as_deref(), Some("mb"));
    }

    /// Every album sharing a deduplicated release group gets the fetch
    /// applied, not just the first one requested.
    #[tokio::test]
    async fn a_shared_release_group_persists_to_every_album_that_uses_it() {
        let mut conn = open_in_memory().unwrap();
        let now = datetime!(2026-08-05 00:00:00 UTC);
        let clock = FixedClock(now);

        upsert_album(&conn, &facts("Albums/A/Artist/One", Some("rg-shared")), now).unwrap();
        upsert_album(&conn, &facts("Albums/B/Artist/Two", Some("rg-shared")), now).unwrap();

        let client = FakeMusicBrainzClient::new(HashMap::from([(
            "rg-shared".to_string(),
            vec![ok_response()],
        )]));

        run_sync(&mut conn, &client, None, false, &clock)
            .await
            .unwrap();

        for path in ["Albums/A/Artist/One", "Albums/B/Artist/Two"] {
            let album = albumdb::find_by_path(&conn, path).unwrap().unwrap();
            assert_eq!(album.mb_sync_state.as_str(), "ok", "{path}");
            assert_eq!(album.primary_genre.as_deref(), Some("dream pop"), "{path}");
        }
    }

    #[tokio::test]
    async fn a_404_is_recorded_as_failed_and_the_run_continues() {
        let mut conn = open_in_memory().unwrap();
        let now = datetime!(2026-08-05 00:00:00 UTC);
        let clock = FixedClock(now);

        upsert_album(&conn, &facts("Albums/A/Artist/Gone", Some("rg-404")), now).unwrap();
        upsert_album(&conn, &facts("Albums/B/Artist/Fine", Some("rg-ok")), now).unwrap();

        let client = FakeMusicBrainzClient::new(HashMap::from([
            ("rg-404".to_string(), vec![Err(MusicBrainzError::NotFound)]),
            ("rg-ok".to_string(), vec![ok_response()]),
        ]));

        let report = run_sync(&mut conn, &client, None, false, &clock)
            .await
            .unwrap();

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
        let mut conn = open_in_memory().unwrap();
        let now = datetime!(2026-08-05 00:00:00 UTC);
        let clock = FixedClock(now);

        upsert_album(&conn, &facts("Albums/A/Artist/Flaky", Some("rg-1")), now).unwrap();
        let client = FakeMusicBrainzClient::new(HashMap::from([(
            "rg-1".to_string(),
            vec![Err(MusicBrainzError::Other(
                "gave up after 3 attempts".to_string(),
            ))],
        )]));

        let report = run_sync(&mut conn, &client, None, false, &clock)
            .await
            .unwrap();
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
        let mut conn = open_in_memory().unwrap();
        let now = datetime!(2026-08-05 00:00:00 UTC);
        let clock = FixedClock(now);

        upsert_album(&conn, &facts("Albums/A/Artist/One", Some("rg-1")), now).unwrap();
        upsert_album(&conn, &facts("Albums/B/Artist/Two", Some("rg-2")), now).unwrap();

        let client = FakeMusicBrainzClient::new(HashMap::from([
            ("rg-1".to_string(), vec![ok_response()]),
            ("rg-2".to_string(), vec![ok_response()]),
        ]));

        let report = run_sync(&mut conn, &client, Some(1), false, &clock)
            .await
            .unwrap();
        assert_eq!(report.release_groups_attempted, 1);
    }

    /// Resumability: a failure already committed on a previous run is never
    /// re-requested by a plain re-run, since it's no longer `pending`.
    /// Simulates "interrupted mid-run" by running twice against the same
    /// database.
    #[tokio::test]
    async fn a_previously_failed_album_is_not_reattempted_without_force() {
        let mut conn = open_in_memory().unwrap();
        let now = datetime!(2026-08-05 00:00:00 UTC);
        let clock = FixedClock(now);

        upsert_album(&conn, &facts("Albums/A/Artist/One", Some("rg-1")), now).unwrap();
        let first_client = FakeMusicBrainzClient::new(HashMap::from([(
            "rg-1".to_string(),
            vec![Err(MusicBrainzError::NotFound)],
        )]));
        run_sync(&mut conn, &first_client, None, false, &clock)
            .await
            .unwrap();

        let second_client = FakeMusicBrainzClient::new(HashMap::new());
        let report = run_sync(&mut conn, &second_client, None, false, &clock)
            .await
            .unwrap();

        assert_eq!(report.release_groups_attempted, 0);
    }

    #[tokio::test]
    async fn force_reattempts_a_previously_failed_album() {
        let mut conn = open_in_memory().unwrap();
        let now = datetime!(2026-08-05 00:00:00 UTC);
        let clock = FixedClock(now);

        upsert_album(&conn, &facts("Albums/A/Artist/One", Some("rg-1")), now).unwrap();
        let first_client = FakeMusicBrainzClient::new(HashMap::from([(
            "rg-1".to_string(),
            vec![Err(MusicBrainzError::NotFound)],
        )]));
        run_sync(&mut conn, &first_client, None, false, &clock)
            .await
            .unwrap();

        let second_client =
            FakeMusicBrainzClient::new(HashMap::from([("rg-1".to_string(), vec![ok_response()])]));
        let report = run_sync(&mut conn, &second_client, None, true, &clock)
            .await
            .unwrap();

        assert_eq!(report.release_groups_attempted, 1);
        assert_eq!(report.albums_fetched_ok, 1);
    }
}
