# MpdController

A macOS menu-bar widget for controlling a local [MPD](https://www.musicpd.org/)
(Music Player Daemon) instance, plus a registered `mpdcontrol://` URL scheme
for fire-and-forget commands from anywhere on the system.

This is a personal tool. Testing and documentation rigour are intentionally
light — enough to be reliable for daily personal use, not production-grade.

- [docs/implementation-details.md](docs/implementation-details.md) — the
  non-obvious things that took real digging (macOS URL-scheme quirks, Dock
  icon, `PATH` under Launch Services, library sharp edges).
- [docs/future_expansions.md](docs/future_expansions.md) — deferred ideas and
  deliberate non-goals.

## How it works

One binary, one long-running process, one crate. Both entry points — the menu
bar and the URL scheme — are handled inside the same process, which owns the
MPD connection and any future in-memory state.

| Module          | Responsibility                                            |
|-----------------|-----------------------------------------------------------|
| `src/main.rs`   | startup, `tao` event loop, tray icon + menu, URL handling  |
| `src/mpd.rs`    | MPD connection, `idle` loop, reconnect/backoff, snapshots, queue/playlist caches |
| `src/command.rs`| decoded command string → dispatch against MPD              |
| `src/menu.rs`   | pure menu model (album nav, playlist membership, copy URIs) + thin render, title formatting/truncation |
| `src/config.rs` | TOML config load + defaults                                |
| `src/clipboard.rs` | pasteboard write (`arboard`), for the copy-URI menu rows |

`command.rs` is the shared dispatch path used by *both* menu clicks and the
URL scheme, so there is one place that knows how to turn `add <path>`,
`toggle`, etc. into MPD operations.

The concurrency split exists because `tray-icon`/`muda` need the `tao` event
loop on the **main thread**, while `mpd_client` is async on **tokio**:

1. The main thread runs the event loop and owns the tray icon and menu.
2. A background tokio task holds the MPD connection and loops on `idle`,
   getting pushed notifications when player/queue state changes.
3. On each change it fetches song + status and sends them to the main thread
   via an `EventLoopProxy`, along with a cheap clone of the `Client` — so the
   main thread always holds a live handle to issue commands with, and no
   separate command channel is needed.
4. Commands (menu click or URL) clone that `Client` and run as tokio tasks, so
   execution never blocks the event loop. If MPD is disconnected, the action
   is a logged no-op rather than an error.

The title and menu refresh **only when MPD state actually changes** —
idle-driven, never polled.

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
key_playlists = [         # default: empty — controls the "Add to" submenu, see below
  "Five stars",
  "My Top Rated",
  "mellow_2024",
]
```

If the file exists but fails to parse, the app fails to start rather than
silently falling back to defaults. A `key_playlists` entry naming a playlist
that doesn't exist in MPD is logged at `warn` and omitted from the menu.

## Using it

The menu bar shows the current track as `Title - Artist` (truncated — the
dropdown's track row always shows it in full), `♫` when connected but
nothing is playing, or `🔇` when MPD is unreachable — the app keeps retrying
with backoff and recovers automatically, no restart needed. When playback is
paused, the title is prefixed with `⏸ ` (outside the truncation budget, so the
track text truncates the same whether playing or paused).

Dropdown menu, top to bottom:

- `⏯ Play/Pause`, `⏭ Next`, `⏹ Clear`
- Album navigation (only while inside a same-directory run of 5+ tracks and
  `random` is off — see `docs/implementation-details.md`):
  - `⏮⏮ Start of Album` on track 2+ of an album, or `⏮⏮ Prev Album: <name>` on
    track 1
  - `⏭⏭ Next Album: <name>`, omitted at the last album
- `⧉ <Title - Artist>` and `⧉ <Album>` — the current track and its album, in
  full. Clicking either copies an `mpdcontrol://add "<path>"` URI to the
  clipboard, ready to paste into Obsidian or anywhere else that takes a link:
  the track's own file for the first, its parent directory (the whole album,
  since `add` is recursive) for the second. The album row is omitted for a
  library-root file, which has no album directory.
- `In playlists ▶` — every playlist containing the current song, disabled
  labels, informational only; omitted when the song is in none
- `Add to ▶` — `key_playlists` minus any already containing the song; clicking
  adds it via `playlistadd`; omitted when `key_playlists` is empty or already
  covers the song
- `Quit`

Separators between sections are omitted when the section between them is
empty.

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

> **Known trade-off (accepted):** this pass-through means *any* `mpdcontrol://`
> link — including one embedded in a web page — can invoke any command,
> including the `mpc` fallback. That's fine for a single-user personal tool;
> it would need a verb allowlist if it ever became multi-user or web-exposed.

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

There is no GUI or integration test harness. Anything touching `muda` is
structurally untestable — `muda::Menu` can only be built on the main thread,
and `cargo test` runs on spawned threads — which is why menu *decisions* live
in `menu::model` rather than `menu::render`. Those paths, and everything
needing a live MPD, were verified by hand (see
[docs/implementation-details.md](docs/implementation-details.md)).
