# Implementation details

Notes for humans, on the things that took real digging to get right. None of
this should need changing again — it's recorded so nobody has to re-derive it,
and so a future change doesn't accidentally undo one of these fixes.

For the shape of the system as a whole, see the README. For what was
deliberately *not* built, see [future_expansions.md](future_expansions.md).

## Why this stack

Chosen after a fresh review of the Rust ecosystem in July 2026.

- **No Tauri.** The only thing that would justify its WebView runtime is a rich
  graphical popover, which we don't want. Native Rust dropdown menus cover
  every need.
- **`tray-icon` + `muda`** are the mature, actively-maintained tray/menu
  choice. The newer `tray` crate is early-stage (v0.1.x, no menu support);
  `tray-item` is weaker for dynamic/nested menus.
- **`tao` over plain `winit`** for the event loop — it's the more
  tray/menu-proven of the two, and is what `tray-icon`/`muda`'s own examples
  use. It also turned out to solve the URL scheme for free (see below).
- **`mpd_client`** — async/tokio, typed responses, and first-class `idle`
  support, which is what makes the push-based (not polled) design possible.

## The URL scheme was the fiddly part

Registering and receiving `mpdcontrol://` looked like it would need custom
macOS plumbing. It didn't, but two non-obvious things had to be discovered.

### `tao` already handles the URL-open event

`tao` installs its own macOS `NSApplicationDelegate` implementing
`application:openURLs:`, and surfaces the result as a plain
`Event::Opened { urls: Vec<url::Url> }` on the same event loop we already run
for the tray. No custom delegate, no hand-rolled `NSAppleEventManager`
(`kAEGetURL`) plumbing, no second process, no polling.

The matured `objc2` ecosystem is still there as a fallback if `Event::Opened`
ever breaks on some future macOS version — but it wasn't needed, and this app
contains no unsafe code (`#![forbid(unsafe_code)]` in `main.rs`).

### The command lands in the URL's *host*, not its path

`mpdcontrol` is a **non-special** URL scheme under the WHATWG URL rules the
`url` crate implements. That means for `mpdcontrol://add%20%22Album%2FPath%22`:

- `url.host_str()` is `Some("add%20%22Album%2FPath%22")` — still
  percent-encoded
- `url.path()` is **empty**

So `decode_command_from_url` in `src/main.rs` reads the *host* and
percent-decodes it. This was confirmed experimentally against that exact
example before anything was built on it, and there's a unit test pinning it.

### Registration only works from `/Applications`

macOS honours `CFBundleURLTypes` only for an app living in `/Applications`,
and only after it has been launched at least once. It cannot be registered at
runtime. Hence `install.sh` and the "launch it once" note in the README —
`cargo run` will never give you a working URL scheme.

## `LSUIElement = true` is not enough to hide the Dock icon

`tao`'s event loop sets `NSApplicationActivationPolicyRegular` at runtime by
default, which overrides the `Info.plist` setting and produces a Dock icon
anyway.

`main.rs` must therefore also call, right after building the event loop:

```rust
event_loop.set_activation_policy(ActivationPolicy::Accessory);
```

(via `tao::platform::macos::EventLoopExtMacOS`). Both halves are needed —
removing either brings the Dock icon back.

Related packaging quirk: `cargo-bundle` has no top-level key for
`LSUIElement`, so it's supplied via
`osx_info_plist_exts = ["assets/info-plist-ext.plist"]`, whose contents get
merged into `Info.plist` verbatim.

## The `mpc` fallback needs absolute Homebrew paths

A GUI app launched via Launch Services — an `mpdcontrol://` link clicked in
Safari or Notes, or a double-click in Finder — inherits **launchd's minimal
`PATH`**, which omits Homebrew's install directories. A bare
`Command::new("mpc")` fails with "No such file or directory" in that context,
even though `mpc` resolves fine from an interactive shell. This is exactly the
kind of bug that never reproduces in development.

`command::resolve_mpc_binary` checks `/opt/homebrew/bin/mpc` and
`/usr/local/bin/mpc` explicitly, falling back to a bare `PATH` lookup (which
does work when running from a dev shell).

## MPD / `mpd_client` sharp edges

- **`pause` doesn't toggle.** MPD's `pause` command takes an explicit on/off
  flag. So the app decides which way to go from its own last-known
  `PlayState`, rather than round-tripping a status query before every click.
  `Stopped` starts playback from the current queue position.
- **`mpd_client`'s typed `Add` wraps `addid`**, which only accepts individual
  files. We want `mpc add` semantics, where a directory path enqueues
  everything under it recursively — so `command.rs` defines its own small
  `Command` impl (`AddRecursive`) wrapping plain `add`.
- **`Song` and `Status` are `#[non_exhaustive]` with no public constructor**,
  so you cannot build one as a test fixture from outside the crate. This is
  why pure formatting/decision logic in `menu.rs` takes plain primitives
  (`Option<&str>`, `&[String]`, …) extracted at the call site rather than the
  `mpd_client` types themselves. Keep new pure logic decoupled the same way or
  it becomes untestable.
- **Cloning `Client` is cheap** — it reuses the underlying connection. That's
  why the MPD task ships a clone along with each state update rather than
  needing a separate command channel back.

## `muda` sharp edges

- `tray_icon::menu` is just a re-export of `muda::*`.
- `muda::MenuItem::new`'s third argument is `Option<Accelerator>`, **not** an
  id. Use `MenuItem::with_id` when you need an id that stays stable across
  rebuilds — which we do, to match click events after `set_menu`.
- `muda` items are `Rc<RefCell<...>>` internally, so rebuilding the whole tray
  menu from scratch on each event (`tray_icon.set_menu(...)`) is fine and
  simpler than mutating one in place. That's what `menu::build_menu` does.

## Two different icons, don't conflate them

- The **tray title is text-only** (`TrayIconBuilder::with_title`) — the track
  text, `♫`, or `🔇`. There is no tray icon image asset.
- **`assets/icon.icns`** is the *app bundle* icon, used by Finder/Dock. It's
  currently a generated placeholder; swapping the file is all that's needed to
  change it, no config edits.

## Verification performed

v1 was verified manually against real MPD, not by an automated integration
harness (deliberately — see the README's note on scope):

- Menu actions: play/pause, next, clear.
- Resilience: killed and restarted MPD mid-session, confirming
  🔇 → backoff → automatic recovery with no app restart, and that the log
  content is genuinely useful for debugging.
- URL scheme end-to-end via real `open 'mpdcontrol://...'` links once packaged
  and installed: a native command (`toggle`), a quoted-path recursive `add`
  (the exact example above), and an `mpc`-fallback command (`volume 50`).

`tray-icon`'s known cosmetic quirks (icon rendering, an occasional double icon
on some macOS versions) were not encountered on the macOS version in use.
