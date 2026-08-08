mod cli;
mod library;
mod mpd_wire;
mod musicbrainz;
mod scan;
mod status;
mod sync;

use std::path::PathBuf;

use anyhow::{Context, Result};
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
            let groups = scan::group_into_albums(songs)?;
            let counts = scan::TagStateCounts::from_groups(&groups);

            if dry_run {
                let conn = albumdb::open(&db_path)?;
                let report = scan::compute_report(&conn, &groups)?;
                println!("scan (dry run, nothing written): {} albums", groups.len());
                println!("{report}");
            } else {
                let mut conn = albumdb::open(&db_path)?;
                let report = scan::write_albums(&mut conn, &groups, &albumdb::SystemClock)?;
                println!(
                    "scan: {} albums written to {}",
                    groups.len(),
                    db_path.display()
                );
                println!("{report}");
            }
            println!("{counts}");
        }
        Command::Sync { limit, force } => {
            let mut conn = albumdb::open(&db_path)?;
            let client =
                musicbrainz::RealMusicBrainzClient::new(musicbrainz::RequestPolicy::default());
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to start async runtime for MusicBrainz requests")?;
            let report = runtime.block_on(sync::run_sync(
                &mut conn,
                &client,
                limit,
                force,
                &albumdb::SystemClock,
            ))?;
            println!("{report}");
        }
        Command::Status => {
            let conn = albumdb::open(&db_path)?;
            let report = status::compute_status_report(&conn)?;
            println!("{report}");
        }
    }

    Ok(())
}

fn resolve_db_path(cli: &Cli) -> Result<PathBuf> {
    if let Some(path) = &cli.db_path {
        return Ok(path.clone());
    }

    let conf = mpd_conf::load(&mpd_conf_path(cli)?)?;
    let music_directory = conf.music_directory.context(
        "music_directory is not set in mpd.conf (it's commented out by default) \
         — required so album-tool can locate albums.db, or pass --db-path",
    )?;
    Ok(music_directory.join(DB_FILENAME))
}

/// `mpd_host` is hardcoded to `localhost` — reading `mpd.conf` off the local
/// filesystem already establishes that MPD is local. The port still comes
/// from `mpd.conf`, independent of `--db-path`, which bypasses only the
/// database *path* derivation: `scan` always needs to reach MPD itself.
fn resolve_mpd_port(cli: &Cli) -> Result<u16> {
    Ok(mpd_conf::load(&mpd_conf_path(cli)?)?.port)
}

fn mpd_conf_path(cli: &Cli) -> Result<PathBuf> {
    match &cli.mpd_conf {
        Some(path) => Ok(path.clone()),
        None => mpd_conf::default_path(),
    }
}
