# MpdController — Implementation Plan

Companion to `spec.md`. Breaks the build into small, independently-completable
and independently-verifiable steps.

## Status: v1 complete

All 13 steps below are done and verified against real MPD (menu actions,
resilience/reconnect, and end-to-end `mpdcontrol://` URL delivery once
packaged and installed). `spec.md` now describes the system as built,
including two runtime gotchas found during verification (macOS Dock icon
despite `LSUIElement`, and the `mpc` fallback needing explicit Homebrew paths)
— see spec §6 and §7 for those.

There is no active next step in this plan. If picking up new work, start from
`spec.md` §10 (deferred menu features: playlist membership/"Add to",
favourites, album traversal, in-memory library cache, sticker metadata) or
§11 (if a CLI ever becomes genuinely necessary) — neither is started. One
implementation detail worth knowing if extending the tray UI: the tray title
is **text-only** (`TrayIconBuilder::with_title`), with no icon image asset —
separate from the **app bundle icon** (`assets/icon.icns`, currently a
placeholder) that Finder/Dock use; don't conflate the two.

---

## Steps (all complete)

1. **Project scaffold & dependencies** — full stack in `Cargo.toml`, template
   scaffold removed, stub modules created, file logging to
   `~/Library/Logs/mpd-controller.log` (level via `RUST_LOG`).
2. **Tray icon + `tao` event loop skeleton** — menu bar shell with a working
   `Quit`, no MPD/tokio yet.
3. **Config loading** — `config.rs`, TOML at
   `~/.config/mpd-controller/config.toml` with defaults; unit-tested.
4. **MPD connection core** — `mpd.rs`: `idle`-driven background tokio task,
   reconnect/backoff (1s→30s cap), pushes state to the main thread via
   `EventLoopProxy`.
5. **Menu-bar title formatting & states** — `menu.rs`: truncation, `♫`/`🔇`
   states; unit-tested.
6. **Core menu actions** — `toggle`/`next`/`clear` wired to menu items.
7. **Generic command parsing** — `command.rs`: decode → `shlex` tokenize →
   dispatch, shared by menu and URL scheme.
8. **`mpc` fallback** — unrecognized commands shell out to `mpc`.
9. **URL scheme handling** — `tao`'s native `Event::Opened` decodes
   `mpdcontrol://` links (opaque host component) into the Step 7/8 dispatch
   path; no custom delegate code needed.
10. **Packaging** — `[package.metadata.bundle]` in `Cargo.toml`
    (`osx_url_schemes`, `osx_info_plist_exts` for `LSUIElement`, icon),
    `install.sh` builds and installs to `/Applications`.
11. **End-to-end URL scheme verification** — confirmed `toggle`, quoted
    recursive `add`, and `mpc`-fallback (`volume 50`) all dispatch correctly
    via real `open mpdcontrol://...` links.
12. **Resilience pass** — killed/restarted MPD mid-session; confirmed
    🔇→backoff→automatic recovery with no app restart, and that log content
    (connection events, reconnect attempts, dispatched commands, errors) is
    genuinely useful for debugging.
13. **README pass** — build/install/config/usage/logging instructions.
