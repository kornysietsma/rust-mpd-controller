use anyhow::{Context, Result, anyhow};
use mpd_client::client::Client;
use mpd_client::commands::{ClearQueue, Command as MpdCommand, Next, Play, SetPause};
use mpd_client::protocol::command::Command as RawCommand;
use mpd_client::protocol::response::Frame;
use mpd_client::responses::{PlayState, TypedResponseError};

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

/// MPD's `add` command, wrapped by hand because `mpd_client`'s typed `Add`
/// wraps `addid` instead, which only accepts a single file. Plain `add`
/// (what `mpc add` also uses) adds a directory recursively, which is the
/// behaviour we want for `add <path>` (spec §7).
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
/// back to something else (e.g. shelling out to `mpc` — spec §7, step 8).
pub enum DispatchOutcome {
    Handled,
    Unrecognized(Vec<String>),
}

/// A decoded command string, tokenized and matched against the commands we
/// know natively. Kept separate from execution so the matching decision
/// (spec §7/§8: native vs. unrecognized) is unit-testable without a live
/// MPD connection.
#[derive(Debug, PartialEq, Eq)]
enum ParsedCommand {
    Toggle,
    Next,
    Clear,
    Add(String),
    Unrecognized(Vec<String>),
}

/// Tokenizes an already percent-decoded command string, respecting quotes
/// (spec §7). Percent-decoding itself happens upstream, at the URL-scheme
/// entry point (step 9) — this function only ever sees plain text.
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

/// The shared dispatch path (spec §7): decoded command text in, matching
/// native command run against `client`. Used directly by the URL scheme
/// (step 9) and exercisable for the actions already wired to menu clicks
/// (step 6) without duplicating their menu-click call sites.
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
}
