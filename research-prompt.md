---
created: 2026-06-08 18:56
importance: 5
tags:
---
I want to write a small app for my personal use to control MPD - the Music Player Daemon - on my local mac.  And I'm trying to decide the tech stack I should use, based on my preferences and the features I'm looking for.
I want to be able to:
- interact with MPD in multiple ways much like the commandline mpc application does - pause, play, enqueue tracks, query the music library, etc
- have a mac toolbar widget which allows me to do some of these - show current track playing, drop-down menus for things like adding the track to a favourites playlist or traversing playing music by album
- this means a long-running service is needed which polls or hooks into mpd updates regularly - so small and light is good
- have a registered uri scheme on the mac - so I could use a uri like `mpcuri://add%20%22Compilations%2FTriple%20J_%20Hottest%20100%2C%20Volume%2023%22` to add a particular album to the current playlist.
- have a cli interface potentially for commands not available in mpc like (one day) adding metadata through mpd's sticker database.

I previously did some of this by hand using [xbar](https://xbarapp.com/) to make toolbar widgets and  [Platypus](https://github.com/sveinbjornt/Platypus) to do uri registrations, but I found both are fragile and error prone. Xbar wraps a cli app with extra widgets - the xbar app I built is at /Users/korny/Dropbox/prj/dev/2024/mpd-xbar/mpd-xbar.1m.rs and can be used as a guide for basic toolbar features I want.

I had a look at some tech options, including using AppleScript or swift natively, but I'd prefer to build this in rust if possible; I'm familiar with rust and it seems to have the features I need.

However some research is probably worthwhile into what libraries are best to use for this. Some suggestions I had from earlier research are:

## 1. Hand-rolled rust
- Package with [`cargo-bundle`](https://github.com/burtonageo/cargo-bundle) (or a hand-rolled bundle) to get the `Info.plist` you need for both the URL scheme and accessory-app status.
- **Menu bar:** [`tray-icon`](https://lib.rs/crates/tray-icon) (the Tauri-ecosystem crate; needs an event loop on the main thread) or the simpler [`tray-item`](https://crates.io/crates/tray-item).
- **MPD:** Use [`mpd_client`](https://lib.rs/crates/mpd_client) (async/tokio, typed responses, good `idle` support) or [`mpd`](https://docs.rs/mpd/) (sync, simpler). The CLI is a separate Rust binary over the same core. Possibly we can fall back to shelling out to `mpc` for commands we don't support natively where speed is less important
- **URL scheme:** register via the bundle's `Info.plist`, but you must catch the Apple Event (`kInternetEventClass` / `kAEGetURL`) yourself — doable but fiddly without an Obj-C bridge, and less documented than Swift's `onOpenURL`.

**Pros:** one language you like; single fast, memory-light binary; reuse the same core for the CLI trivially.
**Cons:** the macOS-native bits are the weak spot — `tray-icon` had open macOS rendering bugs in 2025, dynamic/nested menus are more manual, and URL-event handling is the rough edge. Risk of recreating xbar-style fragility, just in Rust.

## 2. Tauri v2 (Rust backend + web UI) — strong "Rust-first" alternative

Tauri is the framework the `tao`/`tray-icon` crates come from. The backend is Rust (so your MPD logic, CLI, and sticker work stay in Rust), and the UI is HTML/CSS/JS rendered in the system WebView. Crucially, it productises the two fragile features:

- **URL scheme:** [`tauri-plugin-deep-link`](https://crates.io/crates/tauri-plugin-deep-link) generates the `CFBundleURLTypes` entries for you and gives you an `onOpenUrl` callback + `getCurrent()` on launch — the cleanest URL handling of any Rust option. (Same macOS caveat as everyone: must be installed in `/Applications`, can't register at runtime.)
- **Menu bar:** `LSUIElement: true` gives you the accessory app. Tauri has no built-in "menubar mode", but you **don't need a window for plain menus** — see the windowless note below. A frameless webview window is only required if you want a *rich graphical popover* (album art, custom layout).

> [!tip] You can run Tauri windowless with native (Rust) menus
> Tauri v2's tray menus are powered by the **`muda`** crate — the same native-menu library that `tray-icon` re-exports. So you can create **zero windows**, build a `TrayIconBuilder` with a `muda` menu, and handle `on_menu_event`, all in **Rust**. Your dropdowns (album traversal, favourites, a disabled "now playing" label) are native and Rust-defined — *not* HTML/JS. The web UI is only needed if you later want a visual popover panel.
>
> Caveat: going windowless avoids the live `WKWebView` (so you dodge that per-window memory), but Tauri still links its runtime, so the **binary stays Tauri-sized** — lighter than a windowed Tauri app, still heavier than a bare `tray-icon` binary.

**Pros:** backend *and* tray menus stay in Rust; URL scheme is a solved problem via the plugin; bundler generates the `Info.plist` for you; if you ever want a *visual* panel the web UI is there; far lighter than Electron.
**Cons:** still ships the Tauri/WebView runtime (bundle ~3–20 MB, idle RAM ~30–50 MB if a window is shown), vs. a native/`tray-icon` binary at a few MB. If you'll genuinely *only ever* have a dropdown menu, you're carrying a webview runtime you don't use.

> [!info] Where Tauri slots in — and the leaner sibling
> Think of it as a **better Option 2**: staying Rust-first, Tauri beats raw `tao`/`tray-icon` because the deep-link plugin removes the fiddly Apple-Event URL handling *and* the bundler handles the `Info.plist` — at the cost of carrying the runtime. The leaner alternative is **`tray-icon` + `muda` with no Tauri at all**: same native Rust menus and the smallest binary, but then you hand-roll the URL Apple-Event handling and app bundling yourself (and still need a `tao`/`winit` event loop on the main thread — muda/tray-icon can't run without one on macOS). 
