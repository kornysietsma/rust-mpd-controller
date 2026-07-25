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
use std::path::PathBuf;

use anyhow::{Context, Result};
use log::{debug, info};
use simplelog::{Config as LogConfig, LevelFilter, WriteLogger};
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tokio::runtime::Runtime;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{TrayIconBuilder, TrayIconEvent};

use crate::mpd::MpdEvent;

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

fn main() -> Result<()> {
    init_logging()?;
    info!("MpdController starting");

    let config = config::load()?;
    debug!("Loaded config: {config:?}");

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();

    let proxy = event_loop.create_proxy();
    TrayIconEvent::set_event_handler(Some(move |_event| {
        let _ = proxy.send_event(UserEvent::TrayIconEvent);
    }));

    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = proxy.send_event(UserEvent::MenuEvent(event));
    }));

    let quit_item = MenuItem::new("Quit", true, None);
    let quit_id = quit_item.id().clone();

    let tray_menu = Menu::new();
    tray_menu.append(&quit_item)?;

    let tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_title("MpdController")
        .with_tooltip("MpdController")
        .build()?;

    let runtime = Runtime::new().context("failed to start tokio runtime")?;
    let mpd_proxy = event_loop.create_proxy();
    runtime.spawn(mpd::run(config, move |event| {
        let _ = mpd_proxy.send_event(UserEvent::Mpd(event));
    }));

    event_loop.run(move |event, _window_target, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::UserEvent(UserEvent::MenuEvent(menu_event)) if menu_event.id == quit_id => {
                info!("Quit selected, exiting");
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(UserEvent::Mpd(mpd_event)) => {
                let title = match mpd_event {
                    MpdEvent::Connected(snapshot) => {
                        let song = snapshot
                            .song
                            .as_ref()
                            .map(|s| s.title().unwrap_or(&s.url))
                            .unwrap_or("-");
                        format!("{:?}: {song}", snapshot.status.state)
                    }
                    MpdEvent::Disconnected => "(disconnected)".to_string(),
                };
                tray_icon.set_title(Some(title));
            }
            _ => {}
        }
    });
}
