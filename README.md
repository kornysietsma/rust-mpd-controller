# MpdController

A macOS menu-bar widget for controlling a local [MPD](https://www.musicpd.org/)
(Music Player Daemon) instance, plus a registered `mpdcontrol://` URL scheme
for fire-and-forget commands from anywhere on the system.

This is a personal tool — see `spec.md` for the full design and rationale.

## Requirements

- macOS
- Rust (stable, 2024 edition)
- MPD running locally (e.g. `brew install mpd`)
- Optionally, [`mpc`](https://www.musicpd.org/clients/mpc/) on `PATH`
  (`brew install mpc`) — used as a fallback for any command not natively
  implemented (see "URL scheme" below)

## Build & install

```sh
./install.sh
```

This runs `cargo bundle --release` and copies `MpdController.app` into
`/Applications`. macOS only honours the `mpdcontrol://` URL scheme
registration for apps living in `/Applications`, and only after the app has
been launched at least once — so after installing, launch it once from
`/Applications` or Spotlight.

`cargo-bundle` is required to build the bundle:

```sh
cargo install cargo-bundle
```

To run without bundling/installing, during development:

```sh
cargo run
```

(the `mpdcontrol://` URL scheme won't be registered when run this way — that
requires the installed bundle).

## Configuration

Optional TOML file at `~/.config/mpd-controller/config.toml`. Sensible
defaults apply if the file, or individual fields, are missing:

```toml
mpd_host = "localhost"    # default: localhost
mpd_port = 6600           # default: 6600
favourites = "Five stars" # reserved for a future menu feature; no effect yet
key_playlists = [         # reserved for a future menu feature; no effect yet
  "Five stars",
  "My Top Rated",
]
```

If the file exists but fails to parse, the app fails to start rather than
silently falling back to defaults.

## Using it

The menu bar shows the current track as `Title - Artist` (truncated with a
disabled full-text item in the dropdown when long), `♫` when connected but
nothing is playing, or `🔇` when MPD is unreachable — the app keeps retrying
with backoff and recovers automatically, no restart needed.

Dropdown menu: `Play/Pause`, `Next`, `Clear`, `Quit`.

### URL scheme

`mpdcontrol://<command>` dispatches a decoded, `shlex`-tokenised command
against MPD — fire-and-forget, no return value. For example:

```
mpdcontrol://toggle
mpdcontrol://add%20%22Album%2FPath%22
```

`add <path>` is recursive, like `mpc add`. Any command not natively
implemented (`toggle`, `next`, `clear`, `add`) is shelled out to the system
`mpc` binary verbatim, covering the long tail of MPD commands.

## Logging

Logs to `~/Library/Logs/mpd-controller.log`. Default level is `info`;
override with `RUST_LOG` (`trace`, `debug`, `info`, `warn`, `error`, `off`),
e.g.:

```sh
RUST_LOG=debug cargo run
```
