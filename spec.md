# MpdController — Technical Specification

A personal macOS tool to control MPD (Music Player Daemon) running locally. It is
a **single long-running stateful service**: a menu-bar app that also handles a
registered `mpdcontrol://` URL scheme. Rust throughout.

> **Status: v1 complete.** Built, packaged, and verified end-to-end against
> real MPD (see §12). This spec now describes the shipped system, not a plan.
> `plan.md` has the step-by-step build history; §10/§11 below describe
> possible future work, none of it started.

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
| URL scheme      | `tao`'s built-in `Event::Opened` (see §8)  |
| Command parsing | `shlex` (tokenise the decoded URL command, respecting quotes) |
| URL parsing     | `url` + `percent-encoding` (extract/decode the command payload) |
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
- **URL scheme** is the historically fiddly part, but turned out simpler than
  expected: `tao` (chosen over plain `winit` as the more tray/menu-proven of
  the two, per tray-icon/muda's own examples) already installs its own macOS
  `NSApplicationDelegate` implementing `application:openURLs:`, and surfaces
  it as a plain `Event::Opened { urls: Vec<url::Url> }` on the same event loop
  we already run for the tray. No custom delegate or hand-rolled
  `NSAppleEventManager` (`kAEGetURL`) plumbing was needed — see §8 for the
  detail this uncovered about how `url::Url` parses a non-special scheme like
  `mpdcontrol`, and for confirmation that delivery works end-to-end once
  packaged. The matured `objc2` ecosystem remains available as a fallback if
  `Event::Opened` ever proves unreliable in some future macOS version, but
  wasn't needed.
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
3. On each change the task fetches current song + status and sends an update
   to the main thread via an `EventLoopProxy` (custom user event), including a
   cheap clone of the `mpd_client::Client` so the main thread always has a
   live handle to issue commands with — no separate channel needed.
4. The main thread updates the menu-bar title and rebuilds menu items, and
   keeps the latest `Client` (plus last-known play state) as local state for
   dispatching commands.
5. Issuing a command (a menu click, or — once built — a URL-scheme command)
   clones that `Client` and runs the command as an async task on the tokio
   runtime, so command execution never blocks the `tao` event loop. If MPD is
   currently disconnected (no `Client` available yet), the action is a logged
   no-op rather than an error.

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
  Missing individual fields in an otherwise-present file default the same way.
- **Malformed file:** if the file exists but fails to parse, this is treated
  as a real error (the app fails to start) rather than silently falling back
  to defaults — a broken config is a user mistake worth surfacing immediately.

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

> **Implementation note:** `LSUIElement = true` in `Info.plist` is not
> sufficient on its own — `tao`'s event loop sets
> `NSApplicationActivationPolicyRegular` at runtime by default, which
> overrides it and produces a Dock icon anyway. `main.rs` must explicitly call
> `event_loop.set_activation_policy(ActivationPolicy::Accessory)` (via
> `tao::platform::macos::EventLoopExtMacOS`) right after building the event
> loop. Confirmed fixed end-to-end (no Dock icon after rebuild/reinstall).

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

  `Play/Pause` pauses if MPD is currently playing, resumes if paused, and
  starts playback from the current queue position if stopped. MPD's `pause`
  command takes an explicit on/off flag rather than toggling itself, so the
  app decides which way to go from its own last-known play state rather than
  round-tripping a status query before every click.

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
- **`add <path>` is recursive** (directory-aware), matching `mpc add` and
  MPD's native `add` command — so a path to an album folder enqueues every
  track under it. `mpd_client`'s typed `Add` command wraps `addid`, which
  only accepts individual files, so the app defines its own small `Command`
  impl wrapping plain `add` to get this behaviour.
- **Fire-and-forget only.** The URL scheme is one-way; there is no return channel
  to the caller. It is for *commands* (add, play, next, …), not queries. Anything
  that needs to return results is out of scope for this entry point.
- **Optional `mpc` fallback:** a decoded command that isn't natively implemented
  may be shelled out to the system `mpc` binary verbatim, covering the long tail
  of MPD commands for fire-and-forget use. (Implementer's discretion; keep it
  simple.)
  - **Implementation note:** a GUI app launched via Launch Services (e.g. an
    `mpdcontrol://` link opened from Safari/Notes, not a terminal) inherits
    launchd's minimal `PATH`, which omits Homebrew's install dirs — a bare
    `Command::new("mpc")` fails with "No such file or directory" in that
    context even though `mpc` resolves fine from an interactive shell. The
    app checks `/opt/homebrew/bin/mpc` and `/usr/local/bin/mpc` explicitly
    before falling back to a bare `PATH` lookup
    (`command::resolve_mpc_binary`). Confirmed fixed end-to-end.
- **Delivery:** macOS routes the URL to the running **MpdController.app**, which
  handles the URL-open event (`tao`'s built-in `Event::Opened`, see §8) and
  dispatches within its own process/MPD connection. If the app isn't running,
  macOS launches it first.
- **Registration:** via `CFBundleURLTypes` in the bundle `Info.plist` (§9). The
  app must live in `/Applications` for macOS to honour the registration; it
  cannot be registered at runtime.

> **Known trade-off (accepted):** pass-through means *any* `mpdcontrol://` link —
> including one embedded in a web page — can invoke any command, including the
> `mpc` fallback. Acceptable for a single-user personal tool; revisit with a verb
> allowlist if this ever becomes multi-user or web-exposed.

---

## 8. URL-event handling detail

- Handled in-process via `tao`'s built-in `Event::Opened { urls: Vec<url::Url>
  }`, emitted from `tao`'s own macOS `NSApplicationDelegate`
  (`application:openURLs:`) onto the same event loop we already run for the
  tray — no custom delegate code required, no separate process, no polling.
- **Payload extraction:** `mpdcontrol` is a non-special URL scheme (per the
  WHATWG URL rules the `url` crate implements), so everything after `//` in
  e.g. `mpdcontrol://add%20%22Album%2FPath%22` parses into the URL's
  (still percent-encoded) opaque **host** component — `url.path()` is empty.
  The app reads `url.host_str()` and percent-decodes it (`percent-encoding`
  crate) to recover the command text; this was confirmed experimentally
  against the exact spec §7 example before relying on it.
- **Confirmed working end-to-end:** once packaged, installed to
  `/Applications`, and launched once, `open 'mpdcontrol://...'` links are
  delivered via `Event::Opened` and dispatched correctly against real MPD —
  tested with a native command (`toggle`), a quoted-path recursive `add`
  (the exact spec example), and an `mpc`-fallback command (`volume 50`). No
  `objc2`/`NSAppleEventManager` fallback was needed.

---

## 9. Packaging & install

- Build the `.app` with **`cargo-bundle`**, driven by
  `[package.metadata.bundle]` in `Cargo.toml`:
  - App name **MpdController**, identifier `com.sietsma.mpdcontroller`, icon
    (`assets/icon.icns` — currently a generated placeholder; swap the file
    whenever a real icon is wanted, no config changes needed).
  - `LSUIElement = true` via `osx_info_plist_exts = ["assets/info-plist-ext.plist"]`
    (cargo-bundle has no direct top-level key for it, but merges this file's
    contents into `Info.plist` verbatim). See §6's implementation note — this
    alone isn't sufficient; `main.rs` also needs the explicit
    `ActivationPolicy::Accessory` call.
  - `CFBundleURLTypes` declaring the **`mpdcontrol`** scheme, via
    `osx_url_schemes = ["mpdcontrol"]`.
- Install flow: `install.sh` at the repo root runs `cargo bundle --release`
  then copies `MpdController.app` into `/Applications` (replacing any
  existing copy). First launch registers the URL scheme with Launch
  Services.

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
  command parse path, config loading/defaults, and menu display logic (title
  truncation, icon/state selection).
- Pure formatting/decision logic (e.g. `menu.rs`'s title truncation and
  icon-state selection) takes plain primitives (`Option<&str>`, `&[String]`,
  …) rather than `mpd_client` response types directly — those types
  (`Song`, `Status`) are `#[non_exhaustive]` with no public constructor, so
  there's no way to build one as a test fixture outside the crate. Extract the
  fields needed at the call site and keep new pure logic decoupled the same
  way.
- No GUI/integration test harness required.
- A short README with build/install/config instructions is sufficient.

### Status: v1 complete

All sections above describe the system as built and verified against real
MPD, including packaging (§9) and end-to-end URL-scheme delivery (§8) — no
open risks remain from the original plan. Two non-obvious runtime behaviours
were found and fixed during verification and are captured inline above where
relevant: `tao` overriding `LSUIElement` at runtime (§6), and the `mpc`
fallback needing explicit Homebrew paths rather than a bare `PATH` lookup
(§7). `tray-icon`'s known cosmetic quirks (icon rendering, occasional double
icon on some macOS versions) were not encountered during manual verification
on the current OS.

See §10 for deferred features and §11 for how a CLI would fit if ever
genuinely needed — neither is planned or started.
