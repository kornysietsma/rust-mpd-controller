use std::sync::Mutex;
use std::time::{Duration, Instant};

use musicbrainz_rs::entity::release_group::ReleaseGroup;
use musicbrainz_rs::prelude::*;

/// One release-group genre with its MusicBrainz vote count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenreVote {
    pub genre: String,
    pub votes: i64,
}

/// Everything `sync`'s transport layer fetches for one release group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseGroupData {
    pub genres: Vec<GenreVote>,
    /// The track count of the earliest release in the group, stored as
    /// `mb_original_count` — the *tagged* release often carries bonus tracks
    /// and can't supply this. `None` when no release in the group carries
    /// both a date and media/track-count data to derive it from.
    pub earliest_release_track_count: Option<i64>,
}

/// Why a release-group fetch failed. `NotFound` (404) means the MBID is
/// stale and needs a Picard fix — it is never retried. Anything else is a
/// transient-or-unknown failure that already went through the retry/backoff
/// loop and gave up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MusicBrainzError {
    NotFound,
    Other(String),
}

impl std::fmt::Display for MusicBrainzError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MusicBrainzError::NotFound => write!(f, "not found (404): MBID is stale"),
            MusicBrainzError::Other(reason) => write!(f, "{reason}"),
        }
    }
}

/// Fetches release-group data from MusicBrainz. `sync`'s only dependency on
/// the network — a fake implementation stands in for it under test, the same
/// shape as `LibrarySource` for MPD.
pub trait MusicBrainzClient {
    async fn release_group(&self, mbid: &str) -> Result<ReleaseGroupData, MusicBrainzError>;
}

/// A descriptive User-Agent identifying the application and a contact
/// address, as MusicBrainz's API policy requires — anonymous or generic
/// agents get blocked.
pub const USER_AGENT: &str = "album-tool/0.1.0 (korny@sietsma.com)";

/// How long to wait between requests, and how the retry loop backs off on
/// failure. Production uses [`RequestPolicy::default`]; tests pass much
/// shorter durations so they run fast without needing a fake monotonic clock.
#[derive(Debug, Clone, Copy)]
pub struct RequestPolicy {
    pub min_interval: Duration,
    pub backoff_base: Duration,
    pub max_attempts: u32,
}

impl Default for RequestPolicy {
    fn default() -> Self {
        Self {
            min_interval: Duration::from_secs(1),
            backoff_base: Duration::from_secs(2),
            max_attempts: 3,
        }
    }
}

/// Waits at least `min_interval` between successive [`RateLimiter::throttle`]
/// calls. MusicBrainz's policy caps clients at 1 request/second and that has
/// to be enforced here, not hoped for: `musicbrainz_rs` 0.13.0's own
/// leaky-bucket limiter turns out to be unreachable through its public
/// feature flags (AGENTS.md), so this replaces it.
///
/// `wait_duration` is the pure decision (unit-testable without waiting);
/// `throttle` is the thin async wrapper that actually sleeps.
pub struct RateLimiter {
    min_interval: Duration,
    last_request: Mutex<Option<Instant>>,
}

impl RateLimiter {
    pub fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            last_request: Mutex::new(None),
        }
    }

    fn wait_duration(&self, now: Instant) -> Duration {
        match *self.last_request.lock().unwrap() {
            Some(last) => self.min_interval.saturating_sub(now.duration_since(last)),
            None => Duration::ZERO,
        }
    }

    pub async fn throttle(&self) {
        let wait = self.wait_duration(Instant::now());
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        *self.last_request.lock().unwrap() = Some(Instant::now());
    }
}

/// Exponential backoff for the outer retry loop: `base * 2^(attempt - 1)`,
/// so attempt 1 waits `base`, attempt 2 waits `2*base`, attempt 3 waits
/// `4*base`. Pure — the async retry loop that calls it is the thin,
/// untested-directly wrapper (matching `RateLimiter::throttle`).
pub fn backoff_duration(attempt: u32, base: Duration) -> Duration {
    base * 2u32.pow(attempt.saturating_sub(1))
}

/// The real MusicBrainz client, wrapping `musicbrainz_rs`. Handles rate
/// limiting and retry/backoff itself (see [`RateLimiter`]'s doc comment for
/// why) on top of whatever the crate does internally for a bare `503`.
pub struct RealMusicBrainzClient {
    client: musicbrainz_rs::client::MusicBrainzClient,
    rate_limiter: RateLimiter,
    policy: RequestPolicy,
}

impl RealMusicBrainzClient {
    pub fn new(policy: RequestPolicy) -> Self {
        Self {
            client: musicbrainz_rs::client::MusicBrainzClient::new(USER_AGENT),
            rate_limiter: RateLimiter::new(policy.min_interval),
            policy,
        }
    }
}

impl MusicBrainzClient for RealMusicBrainzClient {
    async fn release_group(&self, mbid: &str) -> Result<ReleaseGroupData, MusicBrainzError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            self.rate_limiter.throttle().await;

            match ReleaseGroup::fetch()
                .id(mbid)
                .with_genres()
                .with_releases()
                .with_medias()
                .execute_with_client_async(&self.client)
                .await
            {
                Ok(release_group) => return Ok(to_domain(release_group)),
                Err(err) => {
                    if err.as_musicbrainz_error().is_some_and(|e| e.is_not_found()) {
                        return Err(MusicBrainzError::NotFound);
                    }
                    if attempt >= self.policy.max_attempts {
                        return Err(MusicBrainzError::Other(err.to_string()));
                    }
                    tokio::time::sleep(backoff_duration(attempt, self.policy.backoff_base)).await;
                }
            }
        }
    }
}

/// Just enough of a `musicbrainz_rs::entity::release::Release` to pick the
/// earliest one and total its tracks. Kept separate from that library's own
/// (large, `Default`-less) `Release` type so [`earliest_release_track_count`]
/// stays unit-testable with plain hand-built values, the same reasoning as
/// `SongTags` for MPD.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReleaseTrackInfo {
    /// MusicBrainz's raw date string (`"1972"`, `"1972-02"`, `"1972-02-14"`
    /// — precision varies release to release). Compared lexicographically to
    /// find the earliest, which is correct for same-precision dates and only
    /// approximate across mixed precision (a bare year sorts before any full
    /// date in that year) — an acceptable edge case for this cache column.
    date: String,
    track_count: i64,
}

/// The track count of the earliest release. Callers pass only releases
/// [`release_track_info`] accepted.
fn earliest_release_track_count(releases: &[ReleaseTrackInfo]) -> Option<i64> {
    releases
        .iter()
        .min_by(|a, b| a.date.cmp(&b.date))
        .map(|r| r.track_count)
}

/// Whether one release can contribute to `mb_original_count`, and its total
/// track count if so. Excluded: no date (or a blank one) means it can't be
/// ranked as "earliest"; no media, or media totalling zero tracks, means
/// there is no track count to report — and a zero would otherwise be written
/// out as the nonsense range `1-0`.
///
/// Split out from [`to_domain`] because `musicbrainz_rs::Release` can't be
/// constructed in a test, but these two plain arguments can.
fn release_track_info(
    date: Option<String>,
    media_track_counts: Option<&[i64]>,
) -> Option<ReleaseTrackInfo> {
    let date = date.filter(|d| !d.is_empty())?;
    let track_count: i64 = media_track_counts?.iter().sum();
    (track_count > 0).then_some(ReleaseTrackInfo { date, track_count })
}

fn to_domain(release_group: ReleaseGroup) -> ReleaseGroupData {
    let genres = release_group
        .genres
        .unwrap_or_default()
        .into_iter()
        .map(|g| GenreVote {
            genre: g.name,
            votes: i64::from(g.count.unwrap_or(0)),
        })
        .collect();

    let releases: Vec<ReleaseTrackInfo> = release_group
        .releases
        .unwrap_or_default()
        .into_iter()
        .filter_map(|r| {
            let counts: Option<Vec<i64>> = r
                .media
                .map(|media| media.iter().map(|m| i64::from(m.track_count)).collect());
            release_track_info(r.date.map(|d| d.0), counts.as_deref())
        })
        .collect();

    ReleaseGroupData {
        genres,
        earliest_release_track_count: earliest_release_track_count(&releases),
    }
}

#[cfg(test)]
pub struct FakeMusicBrainzClient {
    responses: std::collections::HashMap<String, Vec<Result<ReleaseGroupData, MusicBrainzError>>>,
    /// How many times each MBID has been called so far — indexes into that
    /// MBID's queued responses.
    call_counts: Mutex<std::collections::HashMap<String, usize>>,
    pub calls: Mutex<Vec<String>>,
}

#[cfg(test)]
impl FakeMusicBrainzClient {
    /// Queues a sequence of responses for one MBID, returned in order —
    /// one entry per call, so a test can script "503 then success" or
    /// "always 503" by queuing enough entries. Calling past the queue's end
    /// panics: a test that hits this has made more requests than it meant
    /// to.
    pub fn new(
        responses: std::collections::HashMap<
            String,
            Vec<Result<ReleaseGroupData, MusicBrainzError>>,
        >,
    ) -> Self {
        Self {
            responses,
            call_counts: Mutex::new(std::collections::HashMap::new()),
            calls: Mutex::new(Vec::new()),
        }
    }
}

#[cfg(test)]
impl MusicBrainzClient for FakeMusicBrainzClient {
    async fn release_group(&self, mbid: &str) -> Result<ReleaseGroupData, MusicBrainzError> {
        self.calls.lock().unwrap().push(mbid.to_string());

        let mut counts = self.call_counts.lock().unwrap();
        let index = counts.entry(mbid.to_string()).or_insert(0);
        let queue = self
            .responses
            .get(mbid)
            .unwrap_or_else(|| panic!("FakeMusicBrainzClient has no queued response for {mbid:?}"));
        let response = queue.get(*index).unwrap_or_else(|| {
            panic!("FakeMusicBrainzClient's queue for {mbid:?} exhausted after {index} calls")
        });
        *index += 1;
        response.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn earliest_release_track_count_picks_the_lowest_date() {
        let releases = vec![
            ReleaseTrackInfo {
                date: "1999-06-01".to_string(),
                track_count: 12,
            },
            ReleaseTrackInfo {
                date: "1972-02-14".to_string(),
                track_count: 10,
            },
            ReleaseTrackInfo {
                date: "1980-01-01".to_string(),
                track_count: 11,
            },
        ];
        assert_eq!(earliest_release_track_count(&releases), Some(10));
    }

    #[test]
    fn earliest_release_track_count_none_when_no_releases_qualify() {
        assert_eq!(earliest_release_track_count(&[]), None);
    }

    #[test]
    fn release_track_info_sums_media_across_discs() {
        assert_eq!(
            release_track_info(Some("1972-02-14".to_string()), Some(&[10, 4])),
            Some(ReleaseTrackInfo {
                date: "1972-02-14".to_string(),
                track_count: 14,
            })
        );
    }

    /// A release that can't be ranked or counted must be dropped, not
    /// defaulted — a zero track count would be written out as `1-0`.
    #[test]
    fn release_track_info_excludes_releases_it_cannot_use() {
        assert_eq!(release_track_info(None, Some(&[10])), None, "no date");
        assert_eq!(
            release_track_info(Some(String::new()), Some(&[10])),
            None,
            "blank date"
        );
        assert_eq!(
            release_track_info(Some("1972".to_string()), None),
            None,
            "no media"
        );
        assert_eq!(
            release_track_info(Some("1972".to_string()), Some(&[])),
            None,
            "empty media"
        );
        assert_eq!(
            release_track_info(Some("1972".to_string()), Some(&[0])),
            None,
            "media totalling zero tracks"
        );
    }

    #[test]
    fn backoff_duration_doubles_each_attempt() {
        let base = Duration::from_millis(10);
        assert_eq!(backoff_duration(1, base), Duration::from_millis(10));
        assert_eq!(backoff_duration(2, base), Duration::from_millis(20));
        assert_eq!(backoff_duration(3, base), Duration::from_millis(40));
    }

    #[test]
    fn rate_limiter_waits_nothing_before_the_first_request() {
        let limiter = RateLimiter::new(Duration::from_secs(1));
        assert_eq!(limiter.wait_duration(Instant::now()), Duration::ZERO);
    }

    #[test]
    fn rate_limiter_computes_remaining_wait_after_a_request() {
        let limiter = RateLimiter::new(Duration::from_millis(100));
        let t0 = Instant::now();
        *limiter.last_request.lock().unwrap() = Some(t0);

        assert_eq!(
            limiter.wait_duration(t0 + Duration::from_millis(40)),
            Duration::from_millis(60)
        );
        assert_eq!(
            limiter.wait_duration(t0 + Duration::from_millis(150)),
            Duration::ZERO
        );
    }

    #[tokio::test]
    async fn throttle_enforces_real_spacing_between_calls() {
        let limiter = RateLimiter::new(Duration::from_millis(30));
        let start = Instant::now();
        limiter.throttle().await;
        limiter.throttle().await;
        limiter.throttle().await;
        assert!(
            start.elapsed() >= Duration::from_millis(60),
            "3 calls at 30ms apart should take at least 60ms, took {:?}",
            start.elapsed()
        );
    }
}
