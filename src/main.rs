#![forbid(unsafe_code)]
#![warn(clippy::all)]
#![warn(rust_2018_idioms)]
#![warn(rust_2024_compatibility)]
#![warn(deprecated_safe)]

mod command;
mod config;
mod menu;
mod mpd;

use std::fs::{self, OpenOptions};
use std::future::Future;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use log::{debug, info, warn};
use mpd_client::client::Client;
use mpd_client::commands::SongId;
use mpd_client::responses::{PlayState, SongInQueue};
use percent_encoding::percent_decode_str;
use simplelog::{Config as LogConfig, LevelFilter, WriteLogger};
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
use tokio::runtime::Runtime;
use tray_icon::menu::MenuEvent;
use tray_icon::{TrayIconBuilder, TrayIconEvent};
use url::Url;

use crate::mpd::MpdEvent;

/// The scheme registered for this app: `mpdcontrol://...`.
const URL_SCHEME: &str = "mpdcontrol";

/// Extracts and percent-decodes the command payload from an `mpdcontrol://`
/// URL. `mpdcontrol` is a non-special URL scheme, so per the
/// WHATWG URL parsing rules the `url` crate implements, everything after
/// `//` lands in the (still percent-encoded) opaque host component, not the
/// path — confirmed against `mpdcontrol://add%20%22Album%2FPath%22`, which
/// parses to host `"add%20%22Album%2FPath%22"` with an empty path.
fn decode_command_from_url(url: &Url) -> Result<String> {
    if url.scheme() != URL_SCHEME {
        return Err(anyhow!(
            "unexpected URL scheme {:?}, expected {URL_SCHEME:?}",
            url.scheme()
        ));
    }
    let payload = url
        .host_str()
        .ok_or_else(|| anyhow!("mpdcontrol URL has no command payload: {url}"))?;
    let decoded = percent_decode_str(payload)
        .decode_utf8()
        .with_context(|| format!("mpdcontrol URL payload is not valid UTF-8: {url}"))?;
    Ok(decoded.into_owned())
}

fn log_file_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME environment variable not set")?;
    let log_dir = PathBuf::from(home).join("Library/Logs");
    fs::create_dir_all(&log_dir)?;
    Ok(log_dir.join("mpd-controller.log"))
}

fn log_level_from_env() -> LevelFilter {
    match std::env::var("RUST_LOG").as_deref() {
        Ok("trace") => LevelFilter::Trace,
        Ok("debug") => LevelFilter::Debug,
        Ok("warn") => LevelFilter::Warn,
        Ok("error") => LevelFilter::Error,
        Ok("off") => LevelFilter::Off,
        _ => LevelFilter::Info,
    }
}

fn init_logging() -> Result<()> {
    let path = log_file_path()?;
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    WriteLogger::init(log_level_from_env(), LogConfig::default(), file)?;
    Ok(())
}

enum UserEvent {
    TrayIconEvent,
    MenuEvent(MenuEvent),
    Mpd(MpdEvent),
}

/// Runs an MPD command on the tokio runtime without blocking the `tao` event
/// loop. A no-op (with a logged warning) if we're not currently connected.
fn spawn_command<F, Fut>(runtime: &Runtime, client: &Option<Client>, label: &'static str, f: F)
where
    F: FnOnce(Client) -> Fut + Send + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    match client {
        Some(client) => {
            let client = client.clone();
            runtime.spawn(async move {
                if let Err(e) = f(client).await {
                    warn!("failed to run {label}: {e:#}");
                }
            });
        }
        None => warn!("ignoring {label}: not connected to MPD"),
    }
}

/// Extracts the primitives `menu::model` needs from MPD's queue — kept here
/// rather than in `menu.rs` so the pure model layer never depends on
/// `mpd_client` types, which can't be constructed in tests. See `AGENTS.md`.
fn queue_entries(queue: &[SongInQueue]) -> Vec<menu::QueueEntry> {
    queue
        .iter()
        .map(|entry| menu::QueueEntry {
            url: entry.song.url.clone(),
            album: entry.song.album().map(str::to_string),
            id: entry.id.0,
        })
        .collect()
}

fn main() -> Result<()> {
    init_logging()?;
    info!("MpdController starting");

    let config = config::load()?;
    debug!("Loaded config: {config:?}");
    // Captured before `config` moves into `mpd::run` below — the event loop
    // closure needs it on every menu rebuild.
    let key_playlists = config.key_playlists.clone();

    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    event_loop.set_activation_policy(ActivationPolicy::Accessory);

    let proxy = event_loop.create_proxy();
    TrayIconEvent::set_event_handler(Some(move |_event| {
        let _ = proxy.send_event(UserEvent::TrayIconEvent);
    }));

    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = proxy.send_event(UserEvent::MenuEvent(event));
    }));

    let tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu::render(&menu::MenuModel::empty())?))
        .with_title("MpdController")
        .with_tooltip("MpdController")
        .build()?;

    let runtime = Runtime::new().context("failed to start tokio runtime")?;
    let mpd_proxy = event_loop.create_proxy();
    runtime.spawn(mpd::run(config, move |event| {
        let _ = mpd_proxy.send_event(UserEvent::Mpd(event));
    }));

    let mut current_client: Option<Client> = None;
    let mut play_state = PlayState::Stopped;
    // The current song's URL, held across events so a click on an `addto:`
    // menu item (which arrives as its own `MenuEvent`, independent of any
    // fresh `MpdEvent`) can act on the song the menu was built from.
    let mut current_url: Option<String> = None;

    event_loop.run(move |event, _window_target, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::UserEvent(UserEvent::MenuEvent(menu_event)) => {
                if menu_event.id == menu::QUIT_ID {
                    info!("Quit selected, exiting");
                    *control_flow = ControlFlow::Exit;
                } else if menu_event.id == menu::PLAY_PAUSE_ID {
                    let state = play_state;
                    spawn_command(
                        &runtime,
                        &current_client,
                        "toggle play/pause",
                        move |c| async move { command::toggle(&c, state).await },
                    );
                } else if menu_event.id == menu::NEXT_ID {
                    spawn_command(&runtime, &current_client, "next", |c| async move {
                        command::next(&c).await
                    });
                } else if menu_event.id == menu::CLEAR_ID {
                    spawn_command(&runtime, &current_client, "clear", |c| async move {
                        command::clear(&c).await
                    });
                } else if let Some(rest) = menu::strip_album_id(&menu_event.id.0) {
                    match rest.parse::<u64>() {
                        Ok(id) => {
                            spawn_command(
                                &runtime,
                                &current_client,
                                "album jump",
                                move |c| async move { command::play_song(&c, SongId(id)).await },
                            );
                        }
                        Err(e) => warn!("ignoring malformed album jump id {rest:?}: {e}"),
                    }
                } else if let Some(name) = menu::strip_addto_id(&menu_event.id.0) {
                    match &current_url {
                        Some(url) => {
                            let name = name.to_string();
                            let url = url.clone();
                            spawn_command(
                                &runtime,
                                &current_client,
                                "add to playlist",
                                move |c| async move {
                                    command::add_to_playlist(&c, &name, &url).await
                                },
                            );
                        }
                        None => warn!("ignoring \"addto:{name}\": no current song"),
                    }
                }
            }
            Event::Opened { urls } => {
                for url in urls {
                    match decode_command_from_url(&url) {
                        Ok(input) => {
                            info!("dispatching mpdcontrol URL command: {input:?}");
                            let state = play_state;
                            spawn_command(
                                &runtime,
                                &current_client,
                                "url command",
                                move |c| async move {
                                    command::dispatch_or_fallback(&c, state, &input).await
                                },
                            );
                        }
                        Err(e) => warn!("ignoring mpdcontrol URL {url}: {e:#}"),
                    }
                }
            }
            Event::UserEvent(UserEvent::Mpd(mpd_event)) => {
                let (display, built_menu) = match mpd_event {
                    MpdEvent::Connected(snapshot, playlists, client) => {
                        debug!("MPD state: {:?}", snapshot.status.state);
                        play_state = snapshot.status.state;
                        current_client = Some(client);
                        current_url = snapshot.song.as_ref().map(|song| song.url.clone());
                        let display = menu::display_for_snapshot(&snapshot);

                        let entries = queue_entries(&snapshot.queue);
                        // Only a position that actually resolves to a queue
                        // entry becomes a `CurrentTrack`; an out-of-bounds one
                        // yields `None`, which is what hides the track section.
                        let current = snapshot
                            .status
                            .current_song
                            .and_then(|(pos, _)| Some((pos.0, entries.get(pos.0)?)))
                            .map(|(pos, entry)| menu::CurrentTrack {
                                pos,
                                url: &entry.url,
                                title: snapshot.song.as_ref().and_then(|song| song.title()),
                                artists: snapshot
                                    .song
                                    .as_ref()
                                    .map_or(&[][..], |song| song.artists()),
                            });
                        let playlists_for_url = snapshot
                            .song
                            .as_ref()
                            .and_then(|song| playlists.membership.get(&song.url))
                            .cloned()
                            .unwrap_or_default();
                        // A name in `key_playlists` that doesn't match a real
                        // MPD playlist is already logged at `warn` when the
                        // cache loads (`mpd.rs::load_playlist_cache`) — this
                        // just excludes it from "Add to".
                        let known_key_playlists: Vec<String> = key_playlists
                            .iter()
                            .filter(|name| playlists.names.contains(name))
                            .cloned()
                            .collect();
                        let model = menu::model(
                            play_state,
                            snapshot.status.random,
                            &entries,
                            current,
                            &playlists_for_url,
                            &known_key_playlists,
                        );
                        (display, menu::render(&model))
                    }
                    MpdEvent::Disconnected => {
                        current_client = None;
                        current_url = None;
                        (
                            menu::disconnected_display(),
                            menu::render(&menu::MenuModel::empty()),
                        )
                    }
                };
                tray_icon.set_title(Some(&display.title));
                match built_menu {
                    Ok(built_menu) => tray_icon.set_menu(Some(Box::new(built_menu))),
                    Err(e) => warn!("failed to rebuild tray menu: {e:#}"),
                }
            }
            _ => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn decodes_quoted_path_from_spec_example() {
        let url = Url::parse("mpdcontrol://add%20%22Album%2FPath%22").unwrap();
        assert_eq!(
            decode_command_from_url(&url).unwrap(),
            r#"add "Album/Path""#
        );
    }

    #[test]
    fn decodes_simple_command_with_no_encoding() {
        let url = Url::parse("mpdcontrol://toggle").unwrap();
        assert_eq!(decode_command_from_url(&url).unwrap(), "toggle");
    }

    #[test]
    fn rejects_unexpected_scheme() {
        let url = Url::parse("https://example.com").unwrap();
        assert!(decode_command_from_url(&url).is_err());
    }
}
