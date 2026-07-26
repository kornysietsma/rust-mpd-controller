# MpdController — Implementation Plan

Companion to `spec.md`. Breaks the build into small, independently-completable
and independently-verifiable steps.

## Context for future steps

- **Local MPD is available** for manual verification throughout — most steps
  include a "verify against real MPD" check, not just unit tests.
- **The full command path is built and working:** `command::dispatch_or_fallback`
  (in `command.rs`) is the single entry point that takes decoded command text,
  tries native commands (`toggle`/`next`/`clear`/`add`), and shells out to the
  system `mpc` binary for anything unrecognized. `main.rs`'s `spawn_command`
  helper runs it on the tokio runtime without blocking the `tao` event loop.
  Both the menu (click IDs) and the URL scheme (Step 9) go through this same
  path — no further plumbing needed here for Steps 10–13.
- **URL scheme handling turned out simpler than the spec originally
  planned:** `tao` already implements macOS URL-open delivery natively as
  `Event::Opened { urls: Vec<url::Url> }` (backed by its own
  `NSApplicationDelegate`) — no custom delegate/`objc2` code was needed. See
  spec §8 for the mechanism and the non-obvious detail that `mpdcontrol`
  being a non-special URL scheme means the command payload lands in
  `url.host_str()` (opaque host), not `url.path()`.
- **Tray title is text-only** (`TrayIconBuilder::with_title`), no icon image
  asset — separate from the **app bundle icon** (Dock/Finder `.icns`) that
  packaging (Step 10, spec §9) will still need; don't conflate the two there.
- **For Step 10 (packaging):** `Cargo.toml`'s `[package.metadata.bundle]`
  needs a `CFBundleURLTypes` entry registering the `mpdcontrol` scheme
  (matches the `URL_SCHEME` constant in `main.rs`), matching spec §9.
- **Gotcha found during Step 10 verification:** `tao`'s event loop defaults to
  `NSApplicationActivationPolicyRegular` on macOS *at runtime*, which overrides
  whatever `LSUIElement` says in `Info.plist` — a Dock icon appeared despite
  `LSUIElement = true` being correctly present in the bundled `Info.plist`.
  Fixed in `main.rs` with an explicit
  `event_loop.set_activation_policy(ActivationPolicy::Accessory)` (via
  `tao::platform::macos::EventLoopExtMacOS`) right after building the event
  loop. Confirmed fixed after rebuild/reinstall — no Dock icon.
- **For Step 11 (end-to-end URL verification):** good link shapes to test
  once installed: `mpdcontrol://toggle` (native), `mpdcontrol://add%20%22Album%2FPath%22`
  (native, quoted path), and something unrecognized like `mpdcontrol://volume%2050`
  (exercises the `mpc` fallback). Check `~/Library/Logs/mpd-controller.log`
  for the dispatched-command line after each.
- Per spec's scope note, manual verification against real MPD substitutes for
  automated/integration tests on several steps — intentional, not a gap.
- **Gotcha found during Step 11 verification:** the `mpc` fallback
  (`command::run_mpc_fallback`) originally shelled out to bare `"mpc"`,
  relying on `PATH`. That works under `cargo run`/a terminal shell, but a GUI
  app launched via Launch Services (e.g. an `mpdcontrol://` link opened from
  Safari/Notes, not a terminal) inherits launchd's minimal `PATH`, which
  omits Homebrew's install dirs — the fallback failed with "No such file or
  directory" until fixed. Fixed via `command::resolve_mpc_binary`, which
  checks `/opt/homebrew/bin/mpc` and `/usr/local/bin/mpc` before falling
  back to a bare `"mpc"` `PATH` lookup. Confirmed fixed end-to-end (volume
  actually changed via the fallback after rebuild/reinstall).

---

## Steps

### 1. Project scaffold & dependencies
- [x] Update `Cargo.toml` with the full stack from spec §3: `tray-icon`,
      `muda`, `tao`, `mpd_client`, `tokio`, `shlex`, `toml`,
      `serde`, a file-logging crate (`simplelog` or similar), plus existing
      `anyhow`/`log`. Drop crates the template added but the spec doesn't need
      (`clap`, `derive_builder`, `lazy_static`, `itertools`) unless something
      later turns out to want them. Make sure dependencies are the most recent stable versions.
- [x] Remove the template scaffold: `src/mymodule.rs`, the `frobnicate`/
      `Opts`/`widgets` demo code in `main.rs`.
- [x] Create empty/stub modules: `src/mpd.rs`, `src/command.rs`,
      `src/config.rs`, `src/menu.rs`, wired up with `mod` declarations.
- [x] Replace `env_logger` with file logging to
      `~/Library/Logs/mpd-controller.log` (create parent dir if missing),
      keeping level configurable via `RUST_LOG`.

**Verify:** `cargo build` succeeds with no warnings from the removed code;
running the (still no-op) binary creates/appends to the log file with a
startup line.

---

### 2. Tray icon + `tao` event loop skeleton
- [x] `main.rs`: `tao` event loop on the main thread; `tray-icon` +
      `muda` menu with a static placeholder title (e.g. `"MpdController"`)
      and a working `Quit` item that exits the loop cleanly.
- [x] No MPD or tokio involved yet — this step only proves the GUI shell
      works.

**Verify (manual):** `cargo run` shows the icon in the macOS menu bar; opening
the dropdown repeatedly doesn't crash or duplicate the icon; `Quit` exits
cleanly.

---

### 3. Config loading
- [x] `config.rs`: `serde`-deriving struct for `mpd_host`, `mpd_port`,
      `favourites`, `key_playlists`, matching spec §5.
- [x] Load from `~/.config/mpd-controller/config.toml`; sensible defaults
      (`localhost`, `6600`, empty lists) when the file or individual fields
      are missing.

**Verify (automated):** unit tests for (a) file absent → defaults, (b)
partial file → missing fields default, (c) full file → all fields parsed
correctly.
**Verify (manual):** drop a real config file at the expected path and confirm
(via a temporary debug log line) the values it produces.

---

### 4. MPD connection core
- [x] `mpd.rs`: connect via `mpd_client` using config values; background
      tokio task looping on `idle`; on each relevant event, fetch `Status` +
      current `Song`.
- [x] Reconnect/backoff on failure (1s → 30s cap per spec §6.1); automatic
      resume of the `idle` loop on recovery.
- [x] Wire into `main.rs`: on state change, send a custom `tao` user event via
      `EventLoopProxy`; update the tray's title with the *raw* data (no
      formatting/truncation yet — this step just proves the data path works
      end-to-end).

**Verify (manual, against local MPD):** play/pause/change track via another
client (e.g. `mpc`) and watch the tray title update in near-real-time without
polling; stop the MPD process and confirm reconnect attempts with backoff
appear in the log and the tray reflects disconnection; restart MPD and
confirm automatic recovery with no app restart needed.

---

### 5. Menu-bar title formatting & states
- [x] `menu.rs`: `"Title - Artist"` formatting, truncation (~25 chars) with
      ellipsis; disabled full-text item shown in the dropdown when truncated.
- [x] State icons: `♫` (connected, nothing playing), `🔇` (disconnected),
      track text otherwise (spec §6).

**Verify (automated):** unit tests for truncation boundary cases (exactly at
limit, just over, unicode-safe truncation) and icon/state selection logic as
pure functions.
**Verify (manual):** confirm all three states render correctly against real
MPD — actively playing, connected with empty queue, MPD killed.

---

### 6. Core menu actions
- [x] `command.rs`: `toggle` (play/pause), `next`, `clear` as functions
      against the MPD connection.
- [x] Wire `Play/Pause`, `Next`, `Clear` menu items to call them.

**Verify (manual):** each menu action changes real MPD playback state as
expected, and the tray updates via the existing idle-driven push (not by
re-polling immediately after the click) — confirms Steps 4–6 are properly
integrated.

---

### 7. Generic command parsing (shared dispatch path)
- [x] `command.rs`: the decode → `shlex` tokenize → dispatch function that
      will be shared by the URL scheme (spec §7) and can also be exercised
      directly for the actions built in Step 6. Recognizes `toggle`, `next`,
      `clear`, `add <path>`, etc.

**Verify (automated):** unit tests for quoting edge cases (spaces, embedded
quotes, empty input), unrecognized command names, and correct tokenisation of
the exact example from spec §7 (`add "Album/Path"`).

---

### 8. `mpc` fallback
- [x] `command.rs`: when `dispatch` (Step 7) returns
      `DispatchOutcome::Unrecognized(tokens)`, shell the tokens out to the
      system `mpc` binary verbatim; log what was dispatched (spec §7, §12).

**Verify (automated):** the native-vs-unrecognized decision itself is
already unit-tested via `parse`'s `Unrecognized` cases (Step 7) — no need to
duplicate that; just test whatever new logic Step 8 adds (e.g. building the
`mpc` argv from the tokens) in isolation from actually invoking `mpc`.
**Verify (manual):** a command not natively implemented (e.g. something only
`mpc` supports) executes correctly against local MPD via the fallback.

---

### 9. URL scheme handling (in-process)
- [x] `main.rs`: handle `tao`'s built-in `Event::Opened` to receive URL-open
      events; percent-decode the payload; pass to the Step 7/8 dispatch path
      (`command::dispatch_or_fallback`). Turned out `tao` already delivers
      this natively — no custom `ApplicationDelegate` code was needed (see
      spec §8).

**Verify (automated):** unit tested the percent-decode step
(`decode_command_from_url` in `main.rs`) in isolation from the OS event
plumbing — covers the spec §7 example, a plain command, and scheme rejection.
**Note:** the OS-level event *delivery* itself still cannot be meaningfully
verified until the URL scheme is registered, which requires packaging (Step
10) — full verification is deferred to Step 11. This is expected, not a gap.

---

### 10. Packaging
- [x] `Cargo.toml`: `[package.metadata.bundle]` — app name `MpdController`,
      icon, `LSUIElement = true`, `CFBundleURLTypes` declaring the
      `mpdcontrol` scheme.
- [x] Install script / `justfile`: `cargo bundle --release` → copy
      `MpdController.app` to `/Applications`.

**Verify (manual):** `cargo bundle --release` succeeds; the app installs and
launches from `/Applications`; menu bar icon appears with no Dock icon
(confirming `LSUIElement`).

---

### 11. End-to-end URL scheme verification
- [x] With the app installed per Step 10, trigger `open mpdcontrol://...` (or
      a link from Safari/Notes) for a few shapes: a native command, an
      `mpc`-fallback command, and a quoted-path `add`.

**Verify (manual):** each link dispatches correctly against real MPD and logs
the dispatched command. If `tao`'s `Event::Opened` delivery proves
unreliable, fall back to `objc2` + `NSAppleEventManager` per spec §8 and
re-verify.

---

### 12. Resilience pass
- [ ] Deliberately kill and restart MPD mid-session; confirm the full cycle
      (🔇 → backoff retries logged → automatic recovery → idle loop resumes)
      with no app restart.
- [ ] Confirm log file content is useful per spec §12: connection events,
      reconnect attempts, dispatched commands, errors.

**Verify (manual):** exercise the above against real MPD; read back the log
file and confirm it would be enough to debug a real issue.

---

### 13. README pass
- [ ] Replace the placeholder README with build/install/config instructions,
      per spec §12 ("a short README... is sufficient" — keep it light).

**Verify (manual):** a fresh read-through of the README is enough to build,
bundle, install, and configure the app from scratch.
