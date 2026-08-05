use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

const DEFAULT_MPD_HOST: &str = "localhost";
const DEFAULT_MPD_PORT: u16 = 6600;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Config {
    pub mpd_host: String,
    pub mpd_port: u16,
    pub key_playlists: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            mpd_host: DEFAULT_MPD_HOST.to_string(),
            mpd_port: DEFAULT_MPD_PORT,
            key_playlists: Vec::new(),
        }
    }
}

fn config_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME environment variable not set")?;
    Ok(PathBuf::from(home).join(".config/mpd-controller/config.toml"))
}

pub fn load() -> Result<Config> {
    load_from(&config_path()?)
}

fn load_from(path: &Path) -> Result<Config> {
    if !path.exists() {
        return Ok(Config::default());
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

    #[test]
    fn defaults_when_file_absent() {
        let path = temp_path("absent");
        assert!(!path.exists());

        let config = load_from(&path).unwrap();

        assert_eq!(config, Config::default());
    }

    #[test]
    fn partial_file_defaults_missing_fields() {
        let path = write_temp("partial", "mpd_host = \"myhost\"\n");

        let config = load_from(&path).unwrap();
        fs::remove_file(&path).ok();

        assert_eq!(config.mpd_host, "myhost");
        assert_eq!(config.mpd_port, DEFAULT_MPD_PORT);
        assert!(config.key_playlists.is_empty());
    }

    #[test]
    fn full_file_parses_all_fields() {
        let path = write_temp(
            "full",
            r#"
                mpd_host = "myhost"
                mpd_port = 1234
                key_playlists = ["Five stars", "My Top Rated"]
            "#,
        );

        let config = load_from(&path).unwrap();
        fs::remove_file(&path).ok();

        assert_eq!(
            config,
            Config {
                mpd_host: "myhost".to_string(),
                mpd_port: 1234,
                key_playlists: vec!["Five stars".to_string(), "My Top Rated".to_string()],
            }
        );
    }

    #[test]
    fn malformed_toml_errors() {
        let path = write_temp("malformed", "this is not valid toml =====");

        let result = load_from(&path);
        fs::remove_file(&path).ok();

        assert!(result.is_err());
    }
}
