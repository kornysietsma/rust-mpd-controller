use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use log::{debug, warn};
use mpd_conf::MpdConf;
use serde::Deserialize;

const DEFAULT_MPD_HOST: &str = "localhost";
const DEFAULT_MPD_PORT: u16 = 6600;
const DB_FILENAME: &str = "albums.db";

/// `~/.config/mpd-controller/config.toml`, as written by a person. Every
/// connection field is optional: unset means "take it from MPD's own
/// `mpd.conf`", which is almost always what's wanted. `key_playlists` has no
/// equivalent there, so it only ever comes from this file.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FileConfig {
    mpd_host: Option<String>,
    mpd_port: Option<u16>,
    /// Where to find MPD's `mpd.conf`. Defaults to `~/.mpd/mpd.conf`.
    mpd_conf: Option<PathBuf>,
    /// Overrides `<music_directory>/albums.db`. A GUI app takes no command
    /// line, so this file is the only place an override can live — the
    /// counterpart to `album-tool --db-path`.
    db_path: Option<PathBuf>,
    key_playlists: Vec<String>,
}

/// The resolved settings the app actually runs on.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub mpd_host: String,
    pub mpd_port: u16,
    /// `None` when neither the config file nor `mpd.conf` could supply one.
    /// Not yet read by anything — the menu-bar app doesn't consume the album
    /// database yet (see `docs/future_expansions.md`).
    pub db_path: Option<PathBuf>,
    pub key_playlists: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            mpd_host: DEFAULT_MPD_HOST.to_string(),
            mpd_port: DEFAULT_MPD_PORT,
            db_path: None,
            key_playlists: Vec::new(),
        }
    }
}

fn config_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME environment variable not set")?;
    Ok(PathBuf::from(home).join(".config/mpd-controller/config.toml"))
}

pub fn load() -> Result<Config> {
    let file = load_file(&config_path()?)?;

    // A missing or unreadable mpd.conf must never stop the app starting —
    // unlike `album-tool`, which is a CLI and can just fail. Fall back to the
    // built-in defaults and say so in the log.
    let mpd_conf_path = match &file.mpd_conf {
        Some(path) => path.clone(),
        None => mpd_conf::default_path()?,
    };
    let conf = match mpd_conf::load(&mpd_conf_path) {
        Ok(conf) => {
            debug!("read MPD settings from {}", mpd_conf_path.display());
            Some(conf)
        }
        Err(e) => {
            warn!(
                "could not read {} ({e:#}); falling back to {DEFAULT_MPD_HOST}:{DEFAULT_MPD_PORT}",
                mpd_conf_path.display()
            );
            None
        }
    };

    Ok(resolve(file, conf.as_ref()))
}

/// Config file wins over `mpd.conf`, which wins over the built-in defaults.
/// Pure, so the precedence rules are testable without touching either file.
fn resolve(file: FileConfig, conf: Option<&MpdConf>) -> Config {
    Config {
        mpd_host: file
            .mpd_host
            .unwrap_or_else(|| DEFAULT_MPD_HOST.to_string()),
        mpd_port: file
            .mpd_port
            .or(conf.map(|c| c.port))
            .unwrap_or(DEFAULT_MPD_PORT),
        db_path: file.db_path.or_else(|| {
            conf.and_then(|c| c.music_directory.as_ref())
                .map(|dir| dir.join(DB_FILENAME))
        }),
        key_playlists: file.key_playlists,
    }
}

fn load_file(path: &Path) -> Result<FileConfig> {
    if !path.exists() {
        return Ok(FileConfig::default());
    }
    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read config file at {}", path.display()))?;
    toml::from_str(&contents)
        .with_context(|| format!("failed to parse config file at {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::io::Write as _;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "mpd-controller-test-{}-{name}-{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn write_temp(name: &str, contents: &str) -> PathBuf {
        let path = temp_path(name);
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        path
    }

    fn mpd_conf() -> MpdConf {
        MpdConf {
            music_directory: Some(PathBuf::from("/music/library")),
            port: 6601,
        }
    }

    #[test]
    fn defaults_when_file_absent() {
        let path = temp_path("absent");
        assert!(!path.exists());

        assert_eq!(load_file(&path).unwrap(), FileConfig::default());
    }

    #[test]
    fn full_file_parses_all_fields() {
        let path = write_temp(
            "full",
            r#"
                mpd_host = "myhost"
                mpd_port = 1234
                mpd_conf = "/etc/mpd.conf"
                db_path = "/elsewhere/albums.db"
                key_playlists = ["Five stars", "My Top Rated"]
            "#,
        );

        let file = load_file(&path).unwrap();
        fs::remove_file(&path).ok();

        assert_eq!(
            file,
            FileConfig {
                mpd_host: Some("myhost".to_string()),
                mpd_port: Some(1234),
                mpd_conf: Some(PathBuf::from("/etc/mpd.conf")),
                db_path: Some(PathBuf::from("/elsewhere/albums.db")),
                key_playlists: vec!["Five stars".to_string(), "My Top Rated".to_string()],
            }
        );
    }

    #[test]
    fn malformed_toml_errors() {
        let path = write_temp("malformed", "this is not valid toml =====");

        let result = load_file(&path);
        fs::remove_file(&path).ok();

        assert!(result.is_err());
    }

    /// A typo in a hand-written config should be reported, not silently
    /// ignored — there's no command line to notice the setting had no effect.
    #[test]
    fn unknown_key_errors() {
        let path = write_temp("unknown", "mpd_prot = 6600\n");

        let result = load_file(&path);
        fs::remove_file(&path).ok();

        assert!(result.is_err());
    }

    #[test]
    fn mpd_conf_supplies_port_and_db_path_when_file_says_nothing() {
        let config = resolve(FileConfig::default(), Some(&mpd_conf()));

        assert_eq!(config.mpd_host, DEFAULT_MPD_HOST);
        assert_eq!(config.mpd_port, 6601);
        assert_eq!(
            config.db_path,
            Some(PathBuf::from("/music/library/albums.db"))
        );
    }

    #[test]
    fn config_file_overrides_mpd_conf() {
        let file = FileConfig {
            mpd_host: Some("elsewhere".to_string()),
            mpd_port: Some(1234),
            db_path: Some(PathBuf::from("/elsewhere/albums.db")),
            ..FileConfig::default()
        };

        let config = resolve(file, Some(&mpd_conf()));

        assert_eq!(config.mpd_host, "elsewhere");
        assert_eq!(config.mpd_port, 1234);
        assert_eq!(config.db_path, Some(PathBuf::from("/elsewhere/albums.db")));
    }

    /// An unreadable mpd.conf must not stop the app starting.
    #[test]
    fn falls_back_to_defaults_without_mpd_conf() {
        assert_eq!(resolve(FileConfig::default(), None), Config::default());
    }

    /// `music_directory` is commented out in a default mpd.conf; the port is
    /// still usable, there is just nowhere to infer the database from.
    #[test]
    fn port_still_resolves_when_music_directory_is_unset() {
        let conf = MpdConf {
            music_directory: None,
            port: 6601,
        };

        let config = resolve(FileConfig::default(), Some(&conf));

        assert_eq!(config.mpd_port, 6601);
        assert_eq!(config.db_path, None);
    }

    /// key_playlists has no mpd.conf equivalent, so it must survive resolution
    /// untouched whether or not mpd.conf was readable.
    #[test]
    fn key_playlists_come_only_from_the_config_file() {
        let file = FileConfig {
            key_playlists: vec!["Five stars".to_string()],
            ..FileConfig::default()
        };

        for conf in [Some(&mpd_conf()), None] {
            let config = resolve(file.clone(), conf);
            assert_eq!(config.key_playlists, vec!["Five stars".to_string()]);
        }
    }
}
