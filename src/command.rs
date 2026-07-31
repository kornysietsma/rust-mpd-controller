use std::path::Path;

use anyhow::{Context, Result, anyhow};
use log::info;
use mpd_client::client::Client;
use mpd_client::commands::{
    AddToPlaylist, ClearQueue, Command as MpdCommand, Next, Play, SetPause, SongId,
};
use mpd_client::protocol::command::Command as RawCommand;
use mpd_client::protocol::response::Frame;
use mpd_client::responses::{PlayState, TypedResponseError};
use tokio::process::Command as TokioCommand;

/// Toggles playback: pauses if playing, resumes if paused, starts playback
/// from the current position if stopped. MPD's `pause` command takes an
/// explicit on/off flag rather than toggling itself, so the caller's last
/// known `PlayState` decides which way to go.
pub async fn toggle(client: &Client, current_state: PlayState) -> Result<()> {
    match current_state {
        PlayState::Playing => client
            .command(SetPause(true))
            .await
            .context("failed to pause")?,
        PlayState::Paused => client
            .command(SetPause(false))
            .await
            .context("failed to resume")?,
        PlayState::Stopped => client
            .command(Play::current())
            .await
            .context("failed to start playback")?,
    };
    Ok(())
}

pub async fn next(client: &Client) -> Result<()> {
    client
        .command(Next)
        .await
        .context("failed to skip to next track")?;
    Ok(())
}

pub async fn clear(client: &Client) -> Result<()> {
    client
        .command(ClearQueue)
        .await
        .context("failed to clear queue")?;
    Ok(())
}

/// Jumps playback to `id` (album nav, menu-only). Uses the queue's `SongId`
/// rather than `SongPosition` since positions shift if the queue is edited
/// between the snapshot the menu was built from and the click.
pub async fn play_song(client: &Client, id: SongId) -> Result<()> {
    client
        .command(Play::song(id))
        .await
        .with_context(|| format!("failed to play song id {}", id.0))?;
    Ok(())
}

/// MPD's `playlistadd <playlist> <song url>` (menu-only "Add to").
pub async fn add_to_playlist(client: &Client, playlist: &str, url: &str) -> Result<()> {
    client
        .command(AddToPlaylist::new(playlist, url))
        .await
        .with_context(|| format!("failed to add {url:?} to playlist {playlist:?}"))?;
    Ok(())
}

/// MPD's `add` command, wrapped by hand because `mpd_client`'s typed `Add`
/// wraps `addid` instead, which only accepts a single file. Plain `add`
/// (what `mpc add` also uses) adds a directory recursively, which is the
/// behaviour we want for `add <path>`.
struct AddRecursive<'a>(&'a str);

impl MpdCommand for AddRecursive<'_> {
    type Response = ();

    fn command(&self) -> RawCommand {
        RawCommand::new("add").argument(self.0)
    }

    fn response(self, _: Frame) -> Result<Self::Response, TypedResponseError> {
        Ok(())
    }
}

pub async fn add(client: &Client, path: &str) -> Result<()> {
    client
        .command(AddRecursive(path))
        .await
        .with_context(|| format!("failed to add {path:?}"))?;
    Ok(())
}

/// Result of dispatching a decoded command string: either it matched a
/// known command and was run, or it didn't and the caller may want to fall
/// back to something else (e.g. shelling out to `mpc`).
pub enum DispatchOutcome {
    Handled,
    Unrecognized(Vec<String>),
}

/// A decoded command string, tokenized and matched against the commands we
/// know natively. Kept separate from execution so the matching decision
/// (native vs. unrecognized) is unit-testable without a live MPD connection.
#[derive(Debug, PartialEq, Eq)]
enum ParsedCommand {
    Toggle,
    Next,
    Clear,
    Add(String),
    Unrecognized(Vec<String>),
}

/// Tokenizes an already percent-decoded command string, respecting quotes.
/// Percent-decoding itself happens upstream, at the URL-scheme entry point
/// in `main.rs` — this function only ever sees plain text.
fn tokenize(input: &str) -> Result<Vec<String>> {
    shlex::split(input)
        .ok_or_else(|| anyhow!("failed to tokenize command (unbalanced quotes?): {input:?}"))
}

fn parse(input: &str) -> Result<ParsedCommand> {
    let tokens = tokenize(input)?;
    let parsed = match tokens.split_first() {
        Some((cmd, [])) if cmd == "toggle" => ParsedCommand::Toggle,
        Some((cmd, [])) if cmd == "next" => ParsedCommand::Next,
        Some((cmd, [])) if cmd == "clear" => ParsedCommand::Clear,
        Some((cmd, [path])) if cmd == "add" => ParsedCommand::Add(path.clone()),
        Some((cmd, _)) if cmd == "add" => {
            return Err(anyhow!(r#""add" takes exactly one path argument"#));
        }
        _ => ParsedCommand::Unrecognized(tokens),
    };
    Ok(parsed)
}

/// The shared dispatch path: decoded command text in, matching native
/// command run against `client`. Used directly by the URL scheme, and
/// exercisable for the actions already wired to menu clicks without
/// duplicating their menu-click call sites.
pub async fn dispatch(
    client: &Client,
    current_state: PlayState,
    input: &str,
) -> Result<DispatchOutcome> {
    match parse(input)? {
        ParsedCommand::Toggle => {
            toggle(client, current_state).await?;
            Ok(DispatchOutcome::Handled)
        }
        ParsedCommand::Next => {
            next(client).await?;
            Ok(DispatchOutcome::Handled)
        }
        ParsedCommand::Clear => {
            clear(client).await?;
            Ok(DispatchOutcome::Handled)
        }
        ParsedCommand::Add(path) => {
            add(client, &path).await?;
            Ok(DispatchOutcome::Handled)
        }
        ParsedCommand::Unrecognized(tokens) => Ok(DispatchOutcome::Unrecognized(tokens)),
    }
}

/// Homebrew's install locations for `mpc`, checked before falling back to a
/// bare `"mpc"` lookup via `PATH`. Needed because a GUI app launched via
/// Launch Services (double-click / `open mpdcontrol://...`, not a terminal)
/// gets launchd's minimal `PATH`, which omits both of these — confirmed via
/// the URL-scheme `mpc` fallback failing with "No such file or directory"
/// despite `mpc` resolving fine from an interactive shell.
const MPC_CANDIDATE_PATHS: &[&str] = &["/opt/homebrew/bin/mpc", "/usr/local/bin/mpc"];

/// Picks the first candidate path that exists, or `"mpc"` to fall back on
/// `PATH` lookup (e.g. when run from a dev shell where it already resolves).
fn resolve_mpc_binary(exists: impl Fn(&str) -> bool) -> &'static str {
    MPC_CANDIDATE_PATHS
        .iter()
        .copied()
        .find(|path| exists(path))
        .unwrap_or("mpc")
}

/// Shells out to the system `mpc` binary for commands `dispatch` doesn't
/// recognize natively, covering the long tail of MPD commands without
/// reimplementing each one. Tokens are passed straight through as `argv`
/// entries (not via a shell), so no quoting/escaping is needed here.
///
/// An empty token list (e.g. empty decoded input) is treated as a no-op
/// rather than invoking bare `mpc`, which would otherwise just print MPD's
/// current status as an unintended side effect.
async fn run_mpc_fallback(tokens: &[String]) -> Result<()> {
    if tokens.is_empty() {
        info!("mpc fallback: nothing to dispatch (empty command), skipping");
        return Ok(());
    }

    info!("dispatching to mpc fallback: {tokens:?}");
    let mpc_binary = resolve_mpc_binary(|path| Path::new(path).exists());
    let output = TokioCommand::new(mpc_binary)
        .args(tokens)
        .output()
        .await
        .context("failed to run mpc")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "mpc exited with {}: {}",
            output.status,
            stderr.trim()
        ));
    }
    Ok(())
}

/// The full command path: tries native dispatch first, falling back to `mpc`
/// for anything unrecognized. This is the entry point the URL scheme calls.
pub async fn dispatch_or_fallback(
    client: &Client,
    current_state: PlayState,
    input: &str,
) -> Result<()> {
    match dispatch(client, current_state, input).await? {
        DispatchOutcome::Handled => Ok(()),
        DispatchOutcome::Unrecognized(tokens) => run_mpc_fallback(&tokens).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn tokenize_splits_on_whitespace() {
        assert_eq!(tokenize("next").unwrap(), vec!["next"]);
    }

    #[test]
    fn tokenize_respects_quotes_with_spaces() {
        assert_eq!(
            tokenize(r#"add "Album/Path""#).unwrap(),
            vec!["add", "Album/Path"]
        );
    }

    #[test]
    fn tokenize_handles_embedded_quotes() {
        assert_eq!(
            tokenize(r#"add "It's a \"Test\"""#).unwrap(),
            vec!["add", r#"It's a "Test""#]
        );
    }

    #[test]
    fn tokenize_empty_input_is_no_tokens() {
        assert_eq!(tokenize("").unwrap(), Vec::<String>::new());
    }

    #[test]
    fn tokenize_rejects_unbalanced_quotes() {
        assert!(tokenize(r#"add "Album/Path"#).is_err());
    }

    #[test]
    fn parse_recognizes_toggle_next_clear() {
        assert_eq!(parse("toggle").unwrap(), ParsedCommand::Toggle);
        assert_eq!(parse("next").unwrap(), ParsedCommand::Next);
        assert_eq!(parse("clear").unwrap(), ParsedCommand::Clear);
    }

    #[test]
    fn parse_recognizes_add_with_quoted_path() {
        assert_eq!(
            parse(r#"add "Album/Path""#).unwrap(),
            ParsedCommand::Add("Album/Path".to_string())
        );
    }

    #[test]
    fn parse_rejects_add_with_wrong_arg_count() {
        assert!(parse("add").is_err());
        assert!(parse("add one two").is_err());
    }

    #[test]
    fn parse_reports_unrecognized_command_names() {
        assert_eq!(
            parse("shuffle").unwrap(),
            ParsedCommand::Unrecognized(vec!["shuffle".to_string()])
        );
    }

    #[test]
    fn parse_reports_empty_input_as_unrecognized() {
        assert_eq!(parse("").unwrap(), ParsedCommand::Unrecognized(vec![]));
    }

    #[tokio::test]
    async fn mpc_fallback_skips_invocation_for_empty_tokens() {
        // No `mpc` binary needs to exist for this to pass: an empty token
        // list returns early rather than shelling out.
        assert!(run_mpc_fallback(&[]).await.is_ok());
    }

    #[test]
    fn resolve_mpc_binary_prefers_first_existing_candidate() {
        assert_eq!(
            resolve_mpc_binary(|path| path == "/opt/homebrew/bin/mpc"),
            "/opt/homebrew/bin/mpc"
        );
        assert_eq!(
            resolve_mpc_binary(|path| path == "/usr/local/bin/mpc"),
            "/usr/local/bin/mpc"
        );
    }

    #[test]
    fn resolve_mpc_binary_falls_back_to_path_lookup() {
        assert_eq!(resolve_mpc_binary(|_| false), "mpc");
    }
}
