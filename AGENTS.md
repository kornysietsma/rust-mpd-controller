# Agent notes

A macOS menu-bar MPD controller plus an `mpdcontrol://` URL scheme. v1 is
complete; see `README.md` for architecture and usage, and
`docs/implementation-details.md` for the non-obvious details behind it.

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
