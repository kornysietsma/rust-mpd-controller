use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use log::debug;
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
    /// `None` when neither the config file nor `mpd.conf` could supply one —
    /// `resolve`'s own possibility, though `load` never returns a `Config`
    /// with this unset: `validate_db_path` turns that case into a
    /// `ConfigError` first. Not yet read by anything — the menu-bar app
    /// doesn't consume the album database yet (see
    /// `docs/future_expansions.md`) — but its *presence* is checked at
    /// startup regardless.
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

/// Everything that can stop the app from reaching a usable `Config`. Each
/// variant carries what a person needs to fix it — the tray's error menu
/// shows `Display`, the log gets the same via `{:#}` on the wrapped error.
/// A GUI app can't just crash on any of these the way `album-tool` (a CLI)
/// does: there's no terminal to see the message on, so the tray has to
/// become the error message instead.
#[derive(Debug)]
pub enum ConfigError {
    /// `HOME` unset, or `config.toml` exists but is unreadable or malformed
    /// (including an unrecognised key).
    ConfigFile(anyhow::Error),
    /// `mpd.conf` — the default location or a `mpd_conf` override — is
    /// missing or unreadable.
    MpdConf {
        path: PathBuf,
        source: anyhow::Error,
    },
    /// Neither `db_path` nor `mpd.conf`'s `music_directory` was set, so
    /// there's nowhere to even look for `albums.db`.
    DbLocationUnknown,
    /// `albums.db`'s parent directory doesn't exist or can't be read.
    DbDirNotAccessible { dir: PathBuf, source: anyhow::Error },
    /// The directory is fine, but `albums.db` itself isn't there yet — not
    /// read by the menu-bar app today, but it will be soon, so a missing
    /// database is caught here rather than surfacing as a mystery later.
    DbFileMissing { path: PathBuf },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::ConfigFile(e) => write!(f, "config.toml error: {e:#}"),
            ConfigError::MpdConf { path, source } => {
                write!(f, "could not read {} ({source:#})", path.display())
            }
            ConfigError::DbLocationUnknown => write!(
                f,
                "album database location unknown — no music_directory in mpd.conf, \
                 and no db_path in config.toml"
            ),
            ConfigError::DbDirNotAccessible { dir, source } => {
                write!(
                    f,
                    "album database directory not accessible: {} ({source:#})",
                    dir.display()
                )
            }
            ConfigError::DbFileMissing { path } => {
                write!(f, "album database not found: {}", path.display())
            }
        }
    }
}

impl std::error::Error for ConfigError {}

fn config_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME environment variable not set")?;
    Ok(PathBuf::from(home).join(".config/mpd-controller/config.toml"))
}

pub fn load() -> Result<Config, ConfigError> {
    let path = config_path().map_err(ConfigError::ConfigFile)?;
    let file = load_file(&path).map_err(ConfigError::ConfigFile)?;

    let mpd_conf_path = match &file.mpd_conf {
        Some(path) => path.clone(),
        None => mpd_conf::default_path().map_err(ConfigError::ConfigFile)?,
    };
    let conf = mpd_conf::load(&mpd_conf_path).map_err(|source| ConfigError::MpdConf {
        path: mpd_conf_path.clone(),
        source,
    })?;
    debug!("read MPD settings from {}", mpd_conf_path.display());

    let config = resolve(file, Some(&conf));
    validate_db_path(&config)?;
    Ok(config)
}

/// Confirms `albums.db` is actually reachable, not just that some path was
/// computed for it. Not read by the menu-bar app yet (see
/// `docs/future_expansions.md`), but that's coming soon, and a broken path
/// is far easier to diagnose at startup than the first time something tries
/// to open it.
fn validate_db_path(config: &Config) -> Result<(), ConfigError> {
    let Some(db_path) = &config.db_path else {
        return Err(ConfigError::DbLocationUnknown);
    };
    let dir = db_path.parent().unwrap_or_else(|| Path::new("."));
    fs::metadata(dir).map_err(|source| ConfigError::DbDirNotAccessible {
        dir: dir.to_path_buf(),
        source: source.into(),
    })?;
    if !db_path.is_file() {
        return Err(ConfigError::DbFileMissing {
            path: db_path.clone(),
        });
    }
    Ok(())
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

    /// `resolve` itself has a defined fallback for "no mpd.conf" — `load`
    /// (untested here; needs real files) is what turns that case into a
    /// `ConfigError::MpdConf` instead of using it.
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

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mpd-controller-test-{}-{name}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn config_with_db_path(db_path: Option<PathBuf>) -> Config {
        Config {
            db_path,
            ..Config::default()
        }
    }

    #[test]
    fn validate_db_path_errors_when_location_unknown() {
        assert!(matches!(
            validate_db_path(&config_with_db_path(None)),
            Err(ConfigError::DbLocationUnknown)
        ));
    }

    #[test]
    fn validate_db_path_errors_when_directory_missing() {
        let dir = temp_dir("dir-missing");
        fs::remove_dir(&dir).unwrap();
        let db_path = dir.join("albums.db");

        assert!(matches!(
            validate_db_path(&config_with_db_path(Some(db_path))),
            Err(ConfigError::DbDirNotAccessible { .. })
        ));
    }

    #[test]
    fn validate_db_path_errors_when_file_missing() {
        let dir = temp_dir("file-missing");
        let db_path = dir.join("albums.db");

        let result = validate_db_path(&config_with_db_path(Some(db_path.clone())));
        fs::remove_dir_all(&dir).ok();

        assert!(matches!(result, Err(ConfigError::DbFileMissing { path }) if path == db_path));
    }

    #[test]
    fn validate_db_path_succeeds_when_file_exists() {
        let dir = temp_dir("file-present");
        let db_path = dir.join("albums.db");
        fs::File::create(&db_path).unwrap();

        let result = validate_db_path(&config_with_db_path(Some(db_path)));
        fs::remove_dir_all(&dir).ok();

        assert!(result.is_ok());
    }
}
