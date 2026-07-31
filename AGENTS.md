# Agent notes

A macOS menu-bar MPD controller plus an `mpdcontrol://` URL scheme. Complete
and in daily use: playback controls, album traversal, playlist membership and
"Add to", copy-URI rows for the current track and album, plus the URL scheme. See `README.md` for architecture and usage, and
`docs/implementation-details.md` for the non-obvious details behind it —
including the measured evidence behind the album-grouping and run-length
rules, which should not be re-derived or "tuned" without re-measuring.

Scope is deliberately personal-tool-sized: prefer simple working code over
exhaustive coverage, and don't design in speculative complexity. Ideas that
were explicitly deferred are in `docs/future_expansions.md` — check there
before proposing "obvious" additions.

## Verifying GUI/tray state

Don't screenshot the screen or scrape the clipboard to check the tray icon
title, menu bar, or other visual state — just ask what's currently shown.
It's faster and more reliable than any programmatic capture.

## Checking crate APIs

Don't guess `mpd_client`/`tray-icon`/`muda` signatures from memory — check the
vendored source first (scoped `find`, not `find /`):

```
find ~/.cargo/registry/src -maxdepth 2 -iname "<crate>-*"
```

Known gotchas already found this way, so you don't have to re-derive them:

- `mpd_client`'s `Song`/`Status` are `#[non_exhaustive]` with no public
  constructor — you can't build one in a test. Keep formatting/display logic
  in terms of plain primitives (`Option<&str>`, `&[String]`, ...) extracted at
  the call site, not the `mpd_client` types themselves, so it stays unit
  testable.
- `tray_icon::menu` is just a re-export of `muda::*`.
- `muda::MenuItem::new`'s third arg is `Option<Accelerator>`, not an id — use
  `MenuItem::with_id` for an id that stays stable across rebuilds (needed to
  match click events after `set_menu`).
- `muda` items are `Rc<RefCell<...>>` internally, so it's fine to rebuild the
  whole tray menu from scratch each event (`tray_icon.set_menu(...)`) rather
  than mutating one in place.
- **`muda::Menu::new` panics off the main thread**, and `cargo test` runs on
  spawned threads — so `menu::render` can never be unit-tested. Don't try.
  Keep decisions in `menu::model` / `MenuModel`'s `show_*_section` methods,
  which are pure and tested.
- Don't add `muda` as a direct dependency — `tray_icon::menu` re-exports it,
  and declaring both risks two resolved muda versions with process-global
  event state. Its absence from `Cargo.toml` is deliberate.
- `mpc lsplaylists` returns each playlist name **twice** (once bare, once with
  `.m3u`) — it's also listing the playlist directory's files, an `mpc`
  artifact. MPD's own `listplaylists` returns clean bare names, which is what
  `mpd_client` sees. Don't add deduplication for this — there's nothing to
  dedupe on the code side.
- `mpd_client` 1.4.1 has no typed wrapper for `listplaylist <name>` (untagged
  filenames only) — only the tagged, far larger `listplaylistinfo`. `mpd.rs`
  hand-rolls it as a `RawCommand`, the same pattern as `command::AddRecursive`
  wrapping `add`.
- A single stored playlist **can** legitimately list the same song twice
  (confirmed live: `Five stars.m3u` does, for Lorde's "Royals") — this one
  *does* need dedup. Sort the playlist names per song, then `Vec::dedup()`
  (adjacent-only, hence the sort first) when building membership.
- `arboard`'s macOS backend has **no `Drop` impl** — a `Clipboard` is just
  `NSPasteboard::generalPasteboard`, so constructing one per copy is correct,
  not wasteful, and there's nothing to keep alive afterwards. Its
  "clipboard empties when the owning process exits" caveat is X11-only.
  `Clipboard::new()` can still fail (`ClipboardNotSupported`) when
  `generalPasteboard` returns NULL, which its source says happens under
  launchd in some daemon modes — handle the error, don't `unwrap`.
- MPD reports `random`/`repeat`/`single`/`consume`/crossfade/replay-gain
  changes on `Subsystem::Options`, **not** `Player` — confirmed live when
  toggling `random` mid-album-run had no effect on the menu until `Options`
  was handled identically to `Player` (both just refetch `Status` +
  `CurrentSong` and reuse the cached queue).

## Load-bearing oddities — don't "clean these up"

Each of these looks redundant but fixes a real bug that only reproduces in the
installed app, not under `cargo run`. Reasons are in
`docs/implementation-details.md`.

- The explicit `set_activation_policy(ActivationPolicy::Accessory)` call in
  `main.rs`, *in addition to* `LSUIElement` in the plist.
- The absolute Homebrew paths in `command::MPC_CANDIDATE_PATHS`.
- `decode_command_from_url` reading the URL's **host**, not its path.
- `command::AddRecursive` hand-wrapping MPD's `add` instead of using
  `mpd_client`'s typed `Add`.
