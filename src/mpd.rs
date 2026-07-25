use std::time::Duration;

use anyhow::{Context, Result};
use log::{info, warn};
use mpd_client::client::{Client, ConnectionEvent, ConnectionEvents, Subsystem};
use mpd_client::commands::{CurrentSong, Status as StatusCommand};
use mpd_client::responses::{Song, Status};
use tokio::net::TcpStream;
use tokio::time::sleep;

use crate::config::Config;

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct PlayerSnapshot {
    pub status: Status,
    pub song: Option<Song>,
}

#[derive(Debug, Clone)]
pub enum MpdEvent {
    /// Also carries a `Client` handle so callers (e.g. menu actions) can
    /// issue commands against the same connection. Cloning `Client` is
    /// cheap — it just reuses the underlying connection.
    Connected(Box<PlayerSnapshot>, Client),
    Disconnected,
}

/// Runs forever: connects to MPD, streams state-change updates via `on_event`, and
/// reconnects with backoff (spec §6.1) on any disconnect.
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

                if let Err(e) = publish_snapshot(&client, &on_event).await {
                    warn!("failed to fetch initial MPD status: {e:#}");
                }

                run_idle_loop(&client, &mut events, &on_event).await;
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

    Client::connect(stream).await.context("MPD handshake failed")
}

async fn run_idle_loop(
    client: &Client,
    events: &mut ConnectionEvents,
    on_event: &impl Fn(MpdEvent),
) {
    loop {
        match events.next().await {
            Some(ConnectionEvent::SubsystemChange(subsystem)) => {
                if matches!(subsystem, Subsystem::Player | Subsystem::Queue)
                    && let Err(e) = publish_snapshot(client, on_event).await
                {
                    warn!("failed to fetch MPD status after state change: {e:#}");
                }
            }
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

async fn publish_snapshot(client: &Client, on_event: &impl Fn(MpdEvent)) -> Result<()> {
    let status = client.command(StatusCommand).await?;
    let song = client.command(CurrentSong).await?.map(|s| s.song);
    on_event(MpdEvent::Connected(
        Box::new(PlayerSnapshot { status, song }),
        client.clone(),
    ));
    Ok(())
}
