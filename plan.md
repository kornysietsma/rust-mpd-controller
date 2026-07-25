# MpdController — Implementation Plan

Companion to `spec.md`. Breaks the build into small, independently-completable
and independently-verifiable steps.

## Technical context & decisions carried into this plan

- **Build order: top-down.** We get a visible tray icon + winit/tao event loop
  running first (Step 2), before any MPD connection exists, then layer in real
  data, then commands, then the URL scheme last (per spec §12, it's the least-
  proven piece — building it last means everything it depends on is already
  solid).
- **Local MPD is available** for manual verification throughout — most steps
  include a "verify against real MPD" check, not just unit tests, consistent
  with the spec's "light testing, reliable for daily use" scope note.
- **`mpc` fallback is in scope for v1** (Step 8), per spec §7.
- **Packaging is in scope** as the final steps (Steps 10–11), because the URL
  scheme genuinely cannot be end-to-end tested without the app bundled,
  `CFBundleURLTypes` registered, and installed in `/Applications` (spec §7, §9).
- **Module layout** follows spec §4 exactly: `main.rs`, `mpd.rs`, `command.rs`,
  `config.rs`, `menu.rs`. `command.rs` stays the single dispatch path used by
  both menu actions and the URL scheme.
- **Concurrency model** follows spec §4: winit/tao event loop + tray on the
  main thread; a background tokio task owns the MPD connection and `idle`
  loop; state changes cross to the main thread via an `EventLoopProxy` custom
  user event. Steps 2 and 4 are where the two halves of this get built and
  then wired together.
- Existing template scaffold (`src/mymodule.rs`, the `frobnicate`/`widgets`
  CLI demo in `main.rs`) is placeholder and gets removed in Step 1, not
  preserved or ported.
- Per spec's scope note, several steps rely on manual verification against a
  running MPD instance rather than automated tests — that's intentional, not
  a gap.

---

## Steps

### 1. Project scaffold & dependencies
- [ ] Update `Cargo.toml` with the full stack from spec §3: `tray-icon`,
      `muda`, `tao` (or `winit`), `mpd_client`, `tokio`, `shlex`, `toml`,
      `serde`, a file-logging crate (`simplelog` or similar), plus existing
      `anyhow`/`log`. Drop crates the template added but the spec doesn't need
      (`clap`, `derive_builder`, `lazy_static`, `itertools`) unless something
      later turns out to want them. Make sure dependencies are the most recent stable versions.
- [ ] Remove the template scaffold: `src/mymodule.rs`, the `frobnicate`/
      `Opts`/`widgets` demo code in `main.rs`.
- [ ] Create empty/stub modules: `src/mpd.rs`, `src/command.rs`,
      `src/config.rs`, `src/menu.rs`, wired up with `mod` declarations.
- [ ] Replace `env_logger` with file logging to
      `~/Library/Logs/mpd-controller.log` (create parent dir if missing),
      keeping level configurable via `RUST_LOG`.

**Verify:** `cargo build` succeeds with no warnings from the removed code;
running the (still no-op) binary creates/appends to the log file with a
startup line.

---

### 2. Tray icon + winit event loop skeleton
- [ ] `main.rs`: winit/tao event loop on the main thread; `tray-icon` +
      `muda` menu with a static placeholder title (e.g. `"MpdController"`)
      and a working `Quit` item that exits the loop cleanly.
- [ ] No MPD or tokio involved yet — this step only proves the GUI shell
      works.

**Verify (manual):** `cargo run` shows the icon in the macOS menu bar; opening
the dropdown repeatedly doesn't crash or duplicate the icon; `Quit` exits
cleanly.

---

### 3. Config loading
- [ ] `config.rs`: `serde`-deriving struct for `mpd_host`, `mpd_port`,
      `favourites`, `key_playlists`, matching spec §5.
- [ ] Load from `~/.config/mpd-controller/config.toml`; sensible defaults
      (`localhost`, `6600`, empty lists) when the file or individual fields
      are missing.

**Verify (automated):** unit tests for (a) file absent → defaults, (b)
partial file → missing fields default, (c) full file → all fields parsed
correctly.
**Verify (manual):** drop a real config file at the expected path and confirm
(via a temporary debug log line) the values it produces.

---

### 4. MPD connection core
- [ ] `mpd.rs`: connect via `mpd_client` using config values; background
      tokio task looping on `idle`; on each relevant event, fetch `Status` +
      current `Song`.
- [ ] Reconnect/backoff on failure (1s → 30s cap per spec §6.1); automatic
      resume of the `idle` loop on recovery.
- [ ] Wire into `main.rs`: on state change, send a custom winit user event via
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
- [ ] `menu.rs`: `"Title - Artist"` formatting, truncation (~25 chars) with
      ellipsis; disabled full-text item shown in the dropdown when truncated.
- [ ] State icons: `♫` (connected, nothing playing), `🔇` (disconnected),
      track text otherwise (spec §6).

**Verify (automated):** unit tests for truncation boundary cases (exactly at
limit, just over, unicode-safe truncation) and icon/state selection logic as
pure functions.
**Verify (manual):** confirm all three states render correctly against real
MPD — actively playing, connected with empty queue, MPD killed.

---

### 6. Core menu actions
- [ ] `command.rs`: `toggle` (play/pause), `next`, `clear` as functions
      against the MPD connection.
- [ ] Wire `Play/Pause`, `Next`, `Clear` menu items to call them.

**Verify (manual):** each menu action changes real MPD playback state as
expected, and the tray updates via the existing idle-driven push (not by
re-polling immediately after the click) — confirms Steps 4–6 are properly
integrated.

---

### 7. Generic command parsing (shared dispatch path)
- [ ] `command.rs`: the decode → `shlex` tokenize → dispatch function that
      will be shared by the URL scheme (spec §7) and can also be exercised
      directly for the actions built in Step 6. Recognizes `toggle`, `next`,
      `clear`, `add <path>`, etc.

**Verify (automated):** unit tests for quoting edge cases (spaces, embedded
quotes, empty input), unrecognized command names, and correct tokenisation of
the exact example from spec §7 (`add "Album/Path"`).

---

### 8. `mpc` fallback
- [ ] `command.rs`: a decoded/tokenized command not natively recognized is
      shelled out to the system `mpc` binary verbatim; log what was
      dispatched (spec §7, §12).

**Verify (automated):** unit test the *dispatch decision* (native vs.
fallback) in isolation from actually invoking `mpc`.
**Verify (manual):** a command not natively implemented (e.g. something only
`mpc` supports) executes correctly against local MPD via the fallback.

---

### 9. URL scheme handling (in-process)
- [ ] `main.rs`: winit custom `ApplicationDelegate` to receive URL-open
      events; percent-decode the payload; pass to the Step 7/8 dispatch path.

**Verify (automated):** unit test the percent-decode step in isolation if
separable from the OS event plumbing.
**Note:** the OS-level event wiring itself cannot be meaningfully verified
until the URL scheme is registered, which requires packaging (Step 10) — full
verification is deferred to Step 11. This is expected, not a gap; flagged as
the highest-risk step per spec §8/§12.

---

### 10. Packaging
- [ ] `Cargo.toml`: `[package.metadata.bundle]` — app name `MpdController`,
      icon, `LSUIElement = true`, `CFBundleURLTypes` declaring the
      `mpdcontrol` scheme.
- [ ] Install script / `justfile`: `cargo bundle --release` → copy
      `MpdController.app` to `/Applications`.

**Verify (manual):** `cargo bundle --release` succeeds; the app installs and
launches from `/Applications`; menu bar icon appears with no Dock icon
(confirming `LSUIElement`).

---

### 11. End-to-end URL scheme verification
- [ ] With the app installed per Step 10, trigger `open mpdcontrol://...` (or
      a link from Safari/Notes) for a few shapes: a native command, an
      `mpc`-fallback command, and a quoted-path `add`.

**Verify (manual):** each link dispatches correctly against real MPD and logs
the dispatched command. If the winit `ApplicationDelegate` approach proves
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
