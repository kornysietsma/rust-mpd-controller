use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use log::{info, warn};
use mpd_client::client::{Client, ConnectionEvent, ConnectionEvents, Subsystem};
use mpd_client::commands::{
    Command as MpdCommand, CurrentSong, GetPlaylists, Queue, Status as StatusCommand,
};
use mpd_client::protocol::command::Command as RawCommand;
use mpd_client::protocol::response::Frame;
use mpd_client::responses::{Song, SongInQueue, Status, TypedResponseError};
use tokio::net::TcpStream;
use tokio::time::sleep;

use crate::config::Config;

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct PlayerSnapshot {
    pub status: Status,
    pub song: Option<Song>,
    pub queue: Vec<SongInQueue>,
}

#[derive(Debug, Clone, Default)]
pub struct PlaylistCache {
    /// song URL -> sorted playlist names containing it.
    pub membership: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone)]
pub enum MpdEvent {
    /// Also carries a `Client` handle so callers (e.g. menu actions) can
    /// issue commands against the same connection. Cloning `Client` is
    /// cheap — it just reuses the underlying connection.
    Connected(Box<PlayerSnapshot>, Box<PlaylistCache>, Client),
    Disconnected,
}

/// `listplaylist <name>` — untagged filenames only. `mpd_client` 1.4.1 has no
/// typed wrapper for this (only the tagged, far larger `listplaylistinfo` via
/// `GetPlaylist`), so it's hand-rolled here, mirroring `command::AddRecursive`.
struct ListPlaylist<'a>(&'a str);

impl MpdCommand for ListPlaylist<'_> {
    type Response = Vec<String>;

    fn command(&self) -> RawCommand {
        RawCommand::new("listplaylist").argument(self.0)
    }

    fn response(self, frame: Frame) -> Result<Self::Response, TypedResponseError> {
        Ok(frame.into_iter().map(|(_, v)| v).collect())
    }
}

/// Runs forever: connects to MPD, streams state-change updates via `on_event`, and
/// reconnects with backoff on any disconnect.
pub async fn run(config: Config, on_event: impl Fn(MpdEvent) + Send + Sync + 'static) {
    let mut backoff = INITIAL_BACKOFF;

    loop {
        match connect(&config).await {
            Ok((client, mut events)) => {
                info!(
                    "connected to MPD at {}:{}",
                    config.mpd_host, config.mpd_port
                );
                backoff = INITIAL_BACKOFF;

                let mut playlist_cache = load_playlist_cache_or_warn(&client).await;
                let mut queue = match publish_full_snapshot(&client, &playlist_cache, &on_event).await
                {
                    Ok(queue) => queue,
                    Err(e) => {
                        warn!("failed to fetch initial MPD status: {e:#}");
                        Vec::new()
                    }
                };

                run_idle_loop(
                    &client,
                    &mut events,
                    &mut queue,
                    &mut playlist_cache,
                    &on_event,
                )
                .await;
            }
            Err(e) => {
                warn!("failed to connect to MPD: {e:#}");
            }
        }

        on_event(MpdEvent::Disconnected);
        info!("retrying MPD connection in {backoff:?}");
        sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

async fn connect(config: &Config) -> Result<(Client, ConnectionEvents)> {
    let stream = TcpStream::connect((config.mpd_host.as_str(), config.mpd_port))
        .await
        .with_context(|| {
            format!(
                "failed to open TCP connection to {}:{}",
                config.mpd_host, config.mpd_port
            )
        })?;

    Client::connect(stream)
        .await
        .context("MPD handshake failed")
}

async fn run_idle_loop(
    client: &Client,
    events: &mut ConnectionEvents,
    queue: &mut Vec<SongInQueue>,
    playlist_cache: &mut PlaylistCache,
    on_event: &impl Fn(MpdEvent),
) {
    loop {
        match events.next().await {
            Some(ConnectionEvent::SubsystemChange(subsystem)) => match subsystem {
                Subsystem::Queue => {
                    match publish_full_snapshot(client, playlist_cache, on_event).await {
                        Ok(fresh_queue) => *queue = fresh_queue,
                        Err(e) => warn!("failed to refetch MPD queue after change: {e:#}"),
                    }
                }
                Subsystem::Player => {
                    if let Err(e) =
                        publish_player_snapshot(client, queue, playlist_cache, on_event).await
                    {
                        warn!("failed to fetch MPD status after state change: {e:#}");
                    }
                }
                Subsystem::StoredPlaylist => {
                    match publish_playlist_snapshot(client, queue, on_event).await {
                        Ok(fresh_cache) => *playlist_cache = fresh_cache,
                        Err(e) => {
                            warn!("failed to fetch MPD status after playlist change: {e:#}");
                        }
                    }
                }
                _ => {}
            },
            Some(ConnectionEvent::ConnectionClosed(err)) => {
                warn!("MPD connection closed: {err}");
                return;
            }
            None => {
                info!("MPD connection closed cleanly");
                return;
            }
        }
    }
}

async fn fetch_status_and_song(client: &Client) -> Result<(Status, Option<Song>)> {
    let status = client.command(StatusCommand).await?;
    let song = client.command(CurrentSong).await?.map(|s| s.song);
    Ok((status, song))
}

async fn fetch_queue(client: &Client) -> Result<Vec<SongInQueue>> {
    client
        .command(Queue::all())
        .await
        .context("failed to fetch queue")
}

/// Loads the full playlist membership cache: all playlist names, then the
/// (untagged) contents of each, folded into a `url -> sorted playlist names`
/// map. Does **not** deduplicate the playlist name list — unlike `mpc
/// lsplaylists`, MPD's own `listplaylists` (what `GetPlaylists` sends)
/// already returns each playlist once.
async fn load_playlist_cache(client: &Client) -> Result<PlaylistCache> {
    let playlists = client
        .command(GetPlaylists)
        .await
        .context("failed to list playlists")?;

    let mut membership: HashMap<String, Vec<String>> = HashMap::new();
    for playlist in playlists {
        let urls = client
            .command(ListPlaylist(&playlist.name))
            .await
            .with_context(|| format!("failed to list playlist {:?}", playlist.name))?;
        for url in urls {
            membership.entry(url).or_default().push(playlist.name.clone());
        }
    }
    for names in membership.values_mut() {
        names.sort();
    }
    Ok(PlaylistCache { membership })
}

/// Loads the playlist cache, warning and falling back to an empty cache on
/// failure rather than propagating — per spec §4.2, the menu should render
/// with empty membership rather than fail outright.
async fn load_playlist_cache_or_warn(client: &Client) -> PlaylistCache {
    load_playlist_cache(client).await.unwrap_or_else(|e| {
        warn!("failed to load playlist cache: {e:#}");
        PlaylistCache::default()
    })
}

/// Refetches `Status`, `CurrentSong`, and the full queue, and publishes them
/// together (reusing the given playlist cache unchanged). Used on connect and
/// on `Subsystem::Queue` changes. Returns the freshly fetched queue so the
/// caller can cache it for subsequent player-only refreshes.
async fn publish_full_snapshot(
    client: &Client,
    playlist_cache: &PlaylistCache,
    on_event: &impl Fn(MpdEvent),
) -> Result<Vec<SongInQueue>> {
    let (status, song) = fetch_status_and_song(client).await?;
    let queue = fetch_queue(client).await?;
    on_event(MpdEvent::Connected(
        Box::new(PlayerSnapshot {
            status,
            song,
            queue: queue.clone(),
        }),
        Box::new(playlist_cache.clone()),
        client.clone(),
    ));
    Ok(queue)
}

/// Refetches only `Status` and `CurrentSong`, reusing the given cached queue
/// and playlist cache. Used on `Subsystem::Player` changes, which never alter
/// queue or playlist contents.
async fn publish_player_snapshot(
    client: &Client,
    queue: &[SongInQueue],
    playlist_cache: &PlaylistCache,
    on_event: &impl Fn(MpdEvent),
) -> Result<()> {
    let (status, song) = fetch_status_and_song(client).await?;
    on_event(MpdEvent::Connected(
        Box::new(PlayerSnapshot {
            status,
            song,
            queue: queue.to_vec(),
        }),
        Box::new(playlist_cache.clone()),
        client.clone(),
    ));
    Ok(())
}

/// Refetches `Status`, `CurrentSong`, and the full playlist cache, reusing the
/// given cached queue (`StoredPlaylist` events never alter queue contents).
/// Used on `Subsystem::StoredPlaylist` changes. Returns the freshly loaded
/// cache so the caller can store it for subsequent refreshes.
async fn publish_playlist_snapshot(
    client: &Client,
    queue: &[SongInQueue],
    on_event: &impl Fn(MpdEvent),
) -> Result<PlaylistCache> {
    let (status, song) = fetch_status_and_song(client).await?;
    let playlist_cache = load_playlist_cache_or_warn(client).await;
    on_event(MpdEvent::Connected(
        Box::new(PlayerSnapshot {
            status,
            song,
            queue: queue.to_vec(),
        }),
        Box::new(playlist_cache.clone()),
        client.clone(),
    ));
    Ok(playlist_cache)
}
