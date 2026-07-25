# Agent notes

Full spec and step-by-step plan live in `spec.md` and `plan.md`. Work through
`plan.md` one step at a time, checking boxes off as you go, and record any
non-obvious decisions made along the way in its "Decisions made while
building" section.

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
