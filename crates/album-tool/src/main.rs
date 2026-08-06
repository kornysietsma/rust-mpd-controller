mod cli;
mod library;
mod mpd_conf;
mod mpd_wire;

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

use cli::{Cli, Command};
use library::{LibrarySource, MpdLibrarySource};

const DB_FILENAME: &str = "albums.db";

fn main() -> Result<()> {
    let cli = Cli::parse();
    let db_path = resolve_db_path(&cli)?;

    match cli.command {
        Command::Scan { dry_run } => {
            let port = resolve_mpd_port(&cli)?;
            let songs = MpdLibrarySource::new("localhost", port).songs()?;
            println!(
                "scan: read {} songs from MPD (db_path={}, dry_run={dry_run}); \
                 grouping and writing not yet implemented",
                songs.len(),
                db_path.display()
            );
        }
        Command::Sync { limit, force } => {
            println!(
                "sync: not yet implemented (db_path={}, limit={limit:?}, force={force})",
                db_path.display()
            );
        }
        Command::Status => {
            println!(
                "status: not yet implemented (db_path={})",
                db_path.display()
            );
        }
    }

    Ok(())
}

fn resolve_db_path(cli: &Cli) -> Result<PathBuf> {
    if let Some(path) = &cli.db_path {
        return Ok(path.clone());
    }

    let conf = mpd_conf::load(&mpd_conf_path(cli)?)?;
    Ok(conf.music_directory.join(DB_FILENAME))
}

/// `mpd_host` is hardcoded to `localhost` (spec §4.1); the port still comes
/// from `mpd.conf`. Independent of `--db-path`, which bypasses only the
/// database *path* derivation — `scan` always needs to reach MPD itself.
fn resolve_mpd_port(cli: &Cli) -> Result<u16> {
    Ok(mpd_conf::load(&mpd_conf_path(cli)?)?.port)
}

fn mpd_conf_path(cli: &Cli) -> Result<PathBuf> {
    match &cli.mpd_conf {
        Some(path) => Ok(path.clone()),
        None => mpd_conf::default_path(),
    }
}
