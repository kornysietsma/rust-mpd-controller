# MpdController

A macOS menu-bar widget for controlling a local [MPD](https://www.musicpd.org/)
(Music Player Daemon) instance, plus a registered `mpdcontrol://` URL scheme
for fire-and-forget commands from anywhere on the system.

> [!WARNING]  
> This is a personal tool! I am sharing it in case others find the code useful, but it
> is deliberately written just for me, it has no security guarantees - in fact
> the URI scheme is quite risky if used without thought as it has no allow-list
> for what it tries to send to MPD.

I'm not planning to maintain this as an open-source tool, it's more of an example of
how you can build personal tools so easily these days with AI tools, it's sometimes
easier to just write the tool you want, than to hunt around for a tool that someone
else maintains.

> [!NOTE]
> Most of this was written by Claude Code. I have applied a human-in-the-loop check
> at all stages, it is not "vibe coded" - but nonetheless I haven't applied as much
> rigour as I would have for commercial code.  If you don't like AI
> written code, I suggest you look elsewhere.

Testing and documentation rigour are intentionally
light — enough to be reliable for daily personal use, not production-grade.

## Main features

This tool creates a macOS application `MpdController.app` which interacts with the MPD music player daemon running on the computer.

It has two interfaces - a toolbar widget which shows currently playing track and album, and has play controls and some playlist integration.

And it also implements a URI scheme so if you click a carefully crafted URI like `mpdcontrol://<command>` it passes the command to the same application for processing.

## Other Docs

- [docs/architecture.md](docs/architecture.md) — module breakdown, the
  main-thread/tokio threading model (with a diagram), and why there's no GUI
  test harness.
- [docs/usage.md](docs/usage.md) — the menu bar walkthrough and the
  `mpdcontrol://` URL scheme reference.
- [docs/implementation-details.md](docs/implementation-details.md) — the
  non-obvious things that took real digging (macOS URL-scheme quirks, Dock
  icon, `PATH` under Launch Services, library sharp edges).
- [docs/album-database.md](docs/album-database.md) — the SQLite album database
  and the `album-tool` CLI that maintains it.
- [docs/future_expansions.md](docs/future_expansions.md) — deferred ideas and
  deliberate non-goals.

## The album database

Alongside the menu-bar app, this repo holds `album-tool` — a CLI that builds a
SQLite database of every album in the MPD library, combining file tags,
MusicBrainz genre data, and hand-entered ratings and category flags. It's
queried by hand in SQL and by agent skills; the menu-bar app doesn't read it
yet. See [docs/album-database.md](docs/album-database.md).

```
album-tool scan [--dry-run]     # read MPD, update the database
album-tool sync [--limit N]     # fetch MusicBrainz release-group data
album-tool status               # read-only report
```

## How it works

One binary, one long-running process: the menu bar and the URL scheme are
handled inside the same process, which owns the MPD connection.
The main thread runs the tray icon and menu (`tao`/`muda`), while a background
tokio task holds the MPD connection and pushes state changes across on every
`idle` notification — see [docs/architecture.md](docs/architecture.md) for
the module breakdown and a diagram of the threading model.

## Requirements

- macOS
- Rust (stable, 2024 edition)
- MPD running locally (e.g. `brew install mpd`)
- [`mpc`](https://www.musicpd.org/clients/mpc/) on `PATH`
  (`brew install mpc`) — used as a fallback for any command not natively
  implemented (see [docs/usage.md](docs/usage.md))

## Build & install

```sh
./install.sh
```

This runs `cargo bundle --release -p controller` and copies `MpdController.app` into
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

Optional TOML file at `~/.config/mpd-controller/config.toml`. **Every
connection setting is inferred from MPD's own `~/.mpd/mpd.conf`**, so in the
normal case the only thing worth putting here is `key_playlists`:

```toml
key_playlists = [         # default: empty — controls the "Add to" submenu, see below
  "Five stars",
  "My Top Rated",
  "Classical Meditation",
]
```

The rest exist only to override that inference:

| Key | Default |
|---|---|
| `mpd_host` | `localhost` — `mpd.conf`'s `bind_to_address` doesn't reliably name a reachable address, so this is never inferred |
| `mpd_port` | `mpd.conf`'s `port`, else 6600 |
| `mpd_conf` | `~/.mpd/mpd.conf` — where to look for MPD's config |
| `db_path` | `<music_directory>/albums.db`, from `mpd.conf`. The counterpart to `album-tool --db-path`; a GUI app takes no command line, so this file is the only place an override can live |

If `mpd.conf` is missing or unreadable the app still starts, logs a warning,
and falls back to `localhost:6600`. If `config.toml` itself exists but fails to
parse — including an unrecognised key, which is usually a typo — the app fails
to start rather than silently ignoring it. A `key_playlists` entry naming a
playlist that doesn't exist in MPD is logged at `warn` and omitted from the
menu.

## Using it

The menu bar shows the current track, with play/next/clear controls, album
navigation, playlist membership, and copy-to-clipboard rows for building
`mpdcontrol://` links. That same URL scheme also works standalone,
fire-and-forget, from anywhere on the system (e.g. an Obsidian link). See
[docs/usage.md](docs/usage.md) for the full menu breakdown and the URL
scheme's syntax, one-command-per-URI limitation, and known issues.

## Logging

Logs to `~/Library/Logs/mpd-controller.log`. Default level is `info`;
override with `RUST_LOG` (`trace`, `debug`, `info`, `warn`, `error`, `off`),
e.g.:

```sh
RUST_LOG=debug cargo run
```

The log covers connection events, reconnect attempts, dispatched commands, and
errors.

## Tests

```sh
cargo test
```

Unit tests only, covering the pure bits that are easy to get wrong: URL decode
→ tokenise → command parse, config loading and defaults, menu display logic
(title truncation, paused prefix, icon/state selection), album traversal
(run detection, jump targets, labels), playlist membership, menu-id
round-tripping, and the copy rows' URI encoding — including a round trip back
out through URL parse → percent-decode → tokenise, which is the only thing
holding the two ends of that format together.

There is no GUI or integration test harness — see
[docs/architecture.md](docs/architecture.md) for why, and
[docs/implementation-details.md](docs/implementation-details.md) for how the
untested paths were verified by hand instead.

## License

[MIT](LICENSE) — take the code or ideas, no permission needed.
