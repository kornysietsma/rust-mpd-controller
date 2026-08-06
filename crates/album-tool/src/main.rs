mod cli;
mod mpd_conf;

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

use cli::{Cli, Command};

const DB_FILENAME: &str = "albums.db";

fn main() -> Result<()> {
    let cli = Cli::parse();
    let db_path = resolve_db_path(&cli)?;

    match cli.command {
        Command::Scan { dry_run } => {
            println!(
                "scan: not yet implemented (db_path={}, dry_run={dry_run})",
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

    let mpd_conf_path = match &cli.mpd_conf {
        Some(path) => path.clone(),
        None => mpd_conf::default_path()?,
    };
    let conf = mpd_conf::load(&mpd_conf_path)?;
    Ok(conf.music_directory.join(DB_FILENAME))
}
