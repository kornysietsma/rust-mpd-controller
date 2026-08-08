//! Reads MPD's own `mpd.conf` off disk, so nothing in this workspace has to
//! duplicate settings MPD already owns.
//!
//! MPD's protocol `config` command is rejected for non-local clients, but both
//! consumers here always run on the same machine as MPD, so reading the file
//! directly sidesteps that entirely.
//!
//! `music_directory` is optional: it is commented out in a default `mpd.conf`,
//! and only a caller that needs to locate `albums.db` cares whether it is set.
//! Deciding that it is *required* is the caller's job, not this crate's — the
//! menu-bar app runs perfectly well without it.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const DEFAULT_PORT: u16 = 6600;

#[derive(Debug, Clone, PartialEq)]
pub struct MpdConf {
    pub music_directory: Option<PathBuf>,
    pub port: u16,
}

pub fn default_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME environment variable not set")?;
    Ok(PathBuf::from(home).join(".mpd/mpd.conf"))
}

pub fn load(path: &Path) -> Result<MpdConf> {
    let contents = fs::read_to_string(path).with_context(|| {
        format!(
            "failed to read MPD config at {} (this should be MPD's own mpd.conf)",
            path.display()
        )
    })?;
    let home = std::env::var("HOME").context("HOME environment variable not set")?;
    parse(&contents, &home)
        .with_context(|| format!("failed to parse MPD config at {}", path.display()))
}

/// Parses MPD's config-file format: `key "value"` / `key value` lines,
/// `#`-to-end-of-line comments, and brace-delimited blocks (`audio_output {
/// ... }`). Only top-level (depth 0) directives are read, so a `port` line
/// nested inside an `audio_output` block is never mistaken for the real one.
fn parse(contents: &str, home: &str) -> Result<MpdConf> {
    let mut music_directory = None;
    let mut port = DEFAULT_PORT;
    let mut depth: i32 = 0;

    for raw_line in contents.lines() {
        let line = strip_comment(raw_line).trim();
        if line.is_empty() {
            depth = (depth + brace_delta(line)).max(0);
            continue;
        }

        if depth == 0
            && let Some((key, value)) = parse_directive(line)
        {
            match key {
                "music_directory" => music_directory = Some(expand_tilde(&value, home)),
                "port" => {
                    port = value
                        .parse()
                        .with_context(|| format!("invalid port value {value:?}"))?
                }
                _ => {}
            }
        }

        depth = (depth + brace_delta(line)).max(0);
    }

    Ok(MpdConf {
        music_directory,
        port,
    })
}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(idx) => &line[..idx],
        None => line,
    }
}

fn brace_delta(line: &str) -> i32 {
    line.matches('{').count() as i32 - line.matches('}').count() as i32
}

fn parse_directive(line: &str) -> Option<(&str, String)> {
    let line = line.trim_end_matches('{').trim();
    let mut parts = line.splitn(2, char::is_whitespace);
    let key = parts.next()?.trim();
    let rest = parts.next()?.trim();
    if key.is_empty() || rest.is_empty() {
        return None;
    }
    Some((key, rest.trim_matches('"').to_string()))
}

fn expand_tilde(value: &str, home: &str) -> PathBuf {
    if value == "~" {
        PathBuf::from(home)
    } else if let Some(rest) = value.strip_prefix("~/") {
        PathBuf::from(home).join(rest)
    } else {
        PathBuf::from(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parses_top_level_directives() {
        let conf = parse(
            r#"
                music_directory "/music/library"
                port "6601"
            "#,
            "/home/korny",
        )
        .unwrap();

        assert_eq!(
            conf,
            MpdConf {
                music_directory: Some(PathBuf::from("/music/library")),
                port: 6601,
            }
        );
    }

    #[test]
    fn port_defaults_when_absent() {
        let conf = parse(r#"music_directory "/music/library""#, "/home/korny").unwrap();

        assert_eq!(conf.port, DEFAULT_PORT);
    }

    #[test]
    fn ignores_commented_lines() {
        let conf = parse(
            r#"
                #music_directory "~/music"
                music_directory "/music/library"
                #port "8000"
                port "6601"
            "#,
            "/home/korny",
        )
        .unwrap();

        assert_eq!(conf.music_directory, Some(PathBuf::from("/music/library")));
        assert_eq!(conf.port, 6601);
    }

    #[test]
    fn ignores_port_nested_in_a_block() {
        let conf = parse(
            r#"
                music_directory "/music/library"
                audio_output {
                    type "httpd"
                    port "8000"
                }
                port "6601"
            "#,
            "/home/korny",
        )
        .unwrap();

        assert_eq!(conf.port, 6601);
    }

    #[test]
    fn expands_tilde_in_music_directory() {
        let conf = parse(r#"music_directory "~/music""#, "/home/korny").unwrap();

        assert_eq!(
            conf.music_directory,
            Some(PathBuf::from("/home/korny/music"))
        );
    }

    /// It's commented out in a default mpd.conf, so absence is normal, not an
    /// error — the port is still usable. Callers that need it say so.
    #[test]
    fn missing_music_directory_is_none_not_an_error() {
        let conf = parse(r#"port "6601""#, "/home/korny").unwrap();

        assert_eq!(conf.music_directory, None);
        assert_eq!(conf.port, 6601);
    }

    #[test]
    fn invalid_port_is_a_clear_error() {
        let result = parse(
            r#"
                music_directory "/music/library"
                port "not-a-number"
            "#,
            "/home/korny",
        );

        assert!(result.is_err());
    }

    #[test]
    fn load_missing_file_is_a_clear_error() {
        let path = std::env::temp_dir().join(format!(
            "mpd-conf-test-nonexistent-{}-{}.conf",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        let result = load(&path);

        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("mpd.conf"),
            "error should mention mpd.conf, was: {err}"
        );
    }

    #[test]
    fn depth_never_goes_negative_on_bare_closing_brace() {
        // A defensive case: malformed input shouldn't panic.
        let conf = parse(
            r#"
                }
                music_directory "/music/library"
            "#,
            "/home/korny",
        )
        .unwrap();

        assert_eq!(conf.music_directory, Some(PathBuf::from("/music/library")));
    }
}
