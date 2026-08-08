use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Maintains the album database derived from the MPD library and MusicBrainz.
#[derive(Debug, Parser)]
#[command(name = "album-tool")]
pub struct Cli {
    /// Path to MPD's own mpd.conf, used to locate the music directory (and
    /// therefore albums.db). Defaults to ~/.mpd/mpd.conf.
    #[arg(long, global = true)]
    pub mpd_conf: Option<PathBuf>,

    /// Override the database path, bypassing mpd.conf entirely.
    #[arg(long, global = true)]
    pub db_path: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Scan the MPD library and update the album database.
    Scan {
        /// Report what would change without writing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Fetch MusicBrainz release-group data for albums pending sync.
    Sync {
        /// Cap the number of requests made this run.
        #[arg(long)]
        limit: Option<usize>,
        /// Re-sync albums that already have mb_sync_state = 'ok'.
        #[arg(long)]
        force: bool,
    },
    /// Print a read-only status report of the album database.
    Status,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("album-tool").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn each_subcommand_parses_with_its_flags() {
        assert!(matches!(
            parse(&["scan"]).command,
            Command::Scan { dry_run: false }
        ));
        assert!(matches!(
            parse(&["scan", "--dry-run"]).command,
            Command::Scan { dry_run: true }
        ));
        assert!(matches!(parse(&["status"]).command, Command::Status));

        match parse(&["sync", "--limit", "5", "--force"]).command {
            Command::Sync { limit, force } => {
                assert_eq!(limit, Some(5));
                assert!(force);
            }
            other => panic!("expected Sync, got {other:?}"),
        }
    }

    /// `global = true` is easy to drop by accident, and losing it would only
    /// show up as a confusing "unexpected argument" at the command line.
    #[test]
    fn global_flags_work_before_or_after_subcommand() {
        let before = parse(&["--db-path", "/tmp/albums.db", "status"]);
        let after = parse(&["status", "--db-path", "/tmp/albums.db"]);

        assert_eq!(before.db_path, Some(PathBuf::from("/tmp/albums.db")));
        assert_eq!(after.db_path, Some(PathBuf::from("/tmp/albums.db")));
    }
}
