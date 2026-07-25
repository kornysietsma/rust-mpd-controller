# MpdController — Technical Specification

A personal macOS tool to control MPD (Music Player Daemon) running locally. It is
a **single long-running stateful service**: a menu-bar app that also handles a
registered `mpdcontrol://` URL scheme. Rust throughout.

> **Scope note:** This is a personal tool. Testing and documentation rigour are
> intentionally light — enough to be reliable for daily personal use, not
> production-grade. Prefer simple, working code over exhaustive coverage.

> **Design principle — YAGNI.** Keep the architecture simple and matched to
> current needs. We deliberately do **not** design in speculative complexity
> (e.g. a CLI, an IPC socket) for futures we may never need. New functionality is
> expected to arrive through the menu bar or the URL scheme; if a genuine need
> for more appears later, §11 sketches how it would fit.

---

## 1. Goals

- A **macOS menu-bar widget** showing the current track with transport controls.
- A **single long-running lightweight service** (the menu-bar app) that reacts to
  MPD state changes in near-real-time and is the natural home for any future
  in-memory state (e.g. a library cache).
- A registered **`mpdcontrol://` URL scheme** for fire-and-forget commands, so
  links like `mpdcontrol://add%20%22Compilations%2FTriple%20J...%22` can enqueue
  music from anywhere on the system.

## 2. Non-goals (v1)

- **No CLI.** The requirements only mentioned one "potentially / one day". `mpc`
  already covers terminal use, and the URL scheme covers programmatic commands.
  See §11 for how a CLI *would* be added if ever actually needed.
- No IPC socket / request-response channel (not needed without a CLI).
- No rich graphical popover / album-art panel — native dropdown menus only (this
  is why we skip Tauri; see §3).
- No playlist-membership UI, album traversal, or favourites action in the menu
  bar yet — deferred (see §10).
- macOS only; personal install only (not packaged for distribution to others).

---

## 3. Tech stack

Chosen after a fresh (July 2026) review of the Rust ecosystem.

| Concern         | Choice                                     |
|-----------------|--------------------------------------------|
| Language        | Rust                                       |
| Menu bar / tray | `tray-icon` + `muda` (native menus)        |
| Event loop      | `tao` (required on main thread)            |
| MPD client      | `mpd_client` (async, tokio, typed, `idle`) |
| Async runtime   | `tokio`                                     |
| URL scheme      | `tao` custom `ApplicationDelegate` (see §8) |
| Command parsing | `shlex` (tokenise the decoded URL command, respecting quotes) |
| Config          | `toml` + `serde`                           |
| Packaging       | `cargo-bundle`                             |
| Logging         | `log` + a file logger (`simplelog` or similar) |
| Errors          | `anyhow`                                    |

### Why this stack (research summary)

- **No Tauri.** The only thing that would justify Tauri's WebView runtime is a
  rich graphical popover, which is out of scope. Native Rust dropdown menus (via
  `muda`, which `tray-icon` re-exports) cover every v1 need.
- **`tray-icon` + `muda`** remains the mature, actively-maintained choice
  (updated mid-2026). The newer `tray` crate is early-stage (v0.1.x, no menu
  support); `tray-item` is weaker for dynamic/nested menus. Known macOS tray
  quirks are cosmetic, not blockers.
- **URL scheme** is the historically fiddly part. Because we already run a
  `tao` event loop on the main thread (required by `tray-icon` on macOS; `tao`
  was chosen over plain `winit` as the more tray/menu-proven of the two, per
  tray-icon/muda's own examples), we handle URL-open events via `tao`'s custom
  `ApplicationDelegate` rather than hand-rolling raw `NSAppleEventManager`
  (`kAEGetURL`) plumbing. The matured `objc2` ecosystem is available as a
  fallback if needed.
- **`mpd_client`** (async/tokio, typed responses, first-class `idle`) is the
  best MPD library for a push-based, event-driven service.

---

## 4. Architecture

**One binary, one long-running process, one crate.** Both entry points (the menu
bar and the URL scheme) are handled inside the same process, which owns the MPD
connection and any future state.

### Crate / module layout

A single binary crate. Internal modules keep concerns separated:

```
rust-mpd-controller/
├── Cargo.toml
├── src/
│   ├── main.rs        # startup, event loop, tray icon + menu, URL-open handling
│   ├── mpd.rs         # MPD connection, idle loop, reconnect/backoff, typed status
│   ├── command.rs     # parse a decoded command string → dispatch against MPD
│   ├── config.rs      # TOML config load + defaults
│   └── menu.rs        # menu construction / title formatting
└── spec.md
```

- **`command.rs`** is the shared dispatch path used by *both* menu actions and
  the URL scheme, so there's one place that knows how to turn "add \<path\>",
  "toggle", etc. into MPD operations. (No separate CLI consumes it — see §2.)

### Concurrency model

`tray-icon`/`muda` need the `tao` event loop on the **main thread**;
`mpd_client` is async on **tokio**. Therefore:

1. Main thread runs the `tao` event loop and owns the tray icon + menu.
2. A background tokio task holds an MPD connection and loops on `idle`,
   receiving push notifications when player/playlist state changes.
3. On each change the task fetches current song + status and sends an update to
   the main thread via an `EventLoopProxy` (custom user event).
4. The main thread updates the menu-bar title and rebuilds menu items.

The menu title/menu refresh **only when MPD state actually changes** (idle-driven,
not polled) — keeps it light.

Because it is a single persistent process, any future in-memory state (e.g. a
library cache) can simply live here, shared by both the menu and URL handling
with no IPC required.

---

## 5. Configuration

A single TOML file; source of truth for connection and playlist settings, with
sensible defaults if the file is missing.

- **Path:** `~/.config/mpd-controller/config.toml`
- **Defaults if absent:** host `localhost`, port `6600`, empty playlist lists.

```toml
mpd_host = "localhost"
mpd_port = 6600
favourites = "Five stars"          # used by deferred favourites feature (§10)
key_playlists = [                   # used by deferred "Add to" feature (§10)
  "Five stars",
  "My Top Rated",
  # ...
]
```

`favourites` and `key_playlists` are parsed in v1 but only consumed by the
deferred menu features; they can be present now without effect.

---

## 6. Menu-bar app (v1)

Bundle: **MpdController.app**, an accessory app (`LSUIElement = true`, no Dock
icon).

### v1 features — Now-playing + transport only

- **Menu-bar title:** current track as `Title - Artist`, truncated (~25 chars)
  with an ellipsis, mirroring the old xbar widget. Full untruncated text shown
  as a (disabled) item in the dropdown when truncated.
- **States:**
  - Playing/paused with a current song → show track text.
  - Connected but nothing playing → show `♫`.
  - Disconnected/unreachable → show `🔇` (see §6.1).
- **Dropdown menu items:**
  - `Play/Pause` (toggle)
  - `Next`
  - `Clear`
  - (disabled) full current-track label when the title is truncated
  - `Quit`

### 6.1 Resilience

- On MPD unreachable: show `🔇`, keep retrying in the background with backoff
  (e.g. 1s → 30s cap).
- On recovery: automatically resume the `idle` loop and normal display.
- The app never needs manual restarting due to MPD going away/restarting.

---

## 7. URL scheme (`mpdcontrol://`) — fire-and-forget commands

- **Interpretation:** pass-through. The text after `//` is URL-decoded, tokenised
  (via `shlex`, respecting quotes), and dispatched through the shared
  `command.rs` path against MPD.
  - Example: `mpdcontrol://add%20%22Album%2FPath%22` → decode →
    `add "Album/Path"` → run.
- **Fire-and-forget only.** The URL scheme is one-way; there is no return channel
  to the caller. It is for *commands* (add, play, next, …), not queries. Anything
  that needs to return results is out of scope for this entry point.
- **Optional `mpc` fallback:** a decoded command that isn't natively implemented
  may be shelled out to the system `mpc` binary verbatim, covering the long tail
  of MPD commands for fire-and-forget use. (Implementer's discretion; keep it
  simple.)
- **Delivery:** macOS routes the URL to the running **MpdController.app**, which
  handles the URL-open event (`tao` `ApplicationDelegate`) and dispatches within
  its own process/MPD connection. If the app isn't running, macOS launches it
  first.
- **Registration:** via `CFBundleURLTypes` in the bundle `Info.plist` (§9). The
  app must live in `/Applications` for macOS to honour the registration; it
  cannot be registered at runtime.

> **Known trade-off (accepted):** pass-through means *any* `mpdcontrol://` link —
> including one embedded in a web page — can invoke any command, including the
> `mpc` fallback. Acceptable for a single-user personal tool; revisit with a verb
> allowlist if this ever becomes multi-user or web-exposed.

---

## 8. URL-event handling detail

- Handled in-process via `tao`'s custom `ApplicationDelegate`, which receives the
  URL when macOS opens it — the event loop we already run for the tray makes this
  the natural mechanism (no separate process, no polling).
- **Fallback if unreliable:** drop to `objc2` and register an
  `NSAppleEventManager` handler for `kInternetEventClass` / `kAEGetURL`,
  extracting the URL from `keyDirectObject`. This is the historically documented
  path and the main risk area (see §12).

---

## 9. Packaging & install

- Build the `.app` with **`cargo-bundle`**, driven by
  `[package.metadata.bundle]` in `Cargo.toml`:
  - App name **MpdController**, icon, `LSUIElement = true`.
  - `CFBundleURLTypes` declaring the **`mpdcontrol`** scheme.
- Install flow (small script / `justfile`):
  1. `cargo bundle --release`
  2. Copy `MpdController.app` to `/Applications`.
  3. First launch registers the URL scheme with Launch Services.
- If `cargo-bundle` can't express something needed (e.g. exact `Info.plist`
  keys), fall back to a small post-build script to patch the bundle —
  implementer's discretion.

---

## 10. Deferred / future menu features

Captured now so the design leaves room; **not** in v1. All fit naturally into the
single stateful service.

- **Menu: playlist membership + "Add to"** — "In playlists" submenu showing which
  `key_playlists` contain the current track, and an "Add to" submenu for the
  rest. (Ports the xbar behaviour; uses `key_playlists` config.)
- **Menu: favourites action** — one-click "add current track to `favourites`".
- **Menu: album traversal** — navigate/browse playing music by album.
- **In-memory library cache** — the payoff of the single-process design; cache
  the library/playlist data in the running service for fast menu building instead
  of re-querying MPD each time.
- **Sticker metadata** — read/write MPD's sticker database (e.g. ratings),
  surfaced through the menu and/or URL scheme.

---

## 11. If a CLI is ever genuinely needed (not now)

Recorded only so the door isn't accidentally closed — **do not build in v1.**

- A CLI process is separate from the running service and cannot see its in-memory
  state directly. For pure MPD passthrough it could just talk to MPD directly
  (like `mpc`); for anything needing the service's cached state it would connect
  over a **Unix domain socket** the service exposes for request/response.
- The URL scheme is *not* a CLI transport: it is one-way (no results returned).
- Most likely we'll never need this — new features are expected via the menu bar
  or URL scheme. This section exists to avoid designing complexity in now.

---

## 12. Logging, testing, risks

### Logging
- File logging (following the xbar precedent) to
  `~/Library/Logs/mpd-controller.log`.
- Reasonable default level; include connection events, reconnect attempts,
  dispatched commands, and errors.

### Testing & docs (light, per scope note)
- Unit-test the pure bits that are easy to get wrong: the URL decode → tokenise →
  command parse path, and config loading/defaults.
- No GUI/integration test harness required.
- A short README with build/install/config instructions is sufficient.

### Open questions / risks
- **`tao` `ApplicationDelegate` URL handling** is the least-proven piece; if it
  proves unreliable, fall back to `objc2` / `NSAppleEventManager` (§8).
- **`tray-icon` macOS cosmetic quirks** (icon rendering, occasional double icon
  on some macOS versions) — expected minor; verify on the current OS.
