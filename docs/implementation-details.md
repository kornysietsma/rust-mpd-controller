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
- **`arboard`** for the clipboard, rather than shelling out to `pbcopy` like
  the `mpc` fallback does. A pipe to `pbcopy` was the first cut, and it's
  worth recording why it lost: spawn, stdin write, and exit status are three
  failure paths instead of one, `child.wait()` has no timeout (a hung
  `pbcopy` would leak the task), and the whole thing needed a tokio task to
  stay off the event loop. `arboard` is two synchronous lines on the main
  thread. The usual objection — a dependency tree for one pipe — doesn't
  apply here: with `default-features = false` its macOS deps are the objc2
  crates `tao`/`tray-icon` already resolve at identical versions, so it costs
  exactly one new crate.

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

### The copy rows write the same format the other end reads

`menu::add_uri` is the only producer of `mpdcontrol://` URIs in the app, and
it has to satisfy two constraints at once:

- **Survive `Url::parse` on the way back in.** The payload lands in the
  opaque host (above), where space, `/`, `:`, `?` and `#` are all forbidden
  raw — so the encode set can't be conservative.
- **Match the links already in Obsidian**, which were generated with Python's
  `quote(s, safe="")`. That leaves exactly RFC 3986's unreserved set
  (`A-Za-z0-9-._~`) alone, which is what `URI_PAYLOAD` spells out, so a
  hand-written `mpcuri://` link and a copied `mpdcontrol://` one are
  byte-identical apart from the scheme.

The path is wrapped in double quotes and its `"`/`\` backslash-escaped, since
the receiving end re-tokenizes with `shlex::split` — an album like
`Weird "Quoted" Album` would otherwise come back as an unbalanced-quote error.
A unit test walks a path containing both characters all the way back out
through URL parse → percent-decode → tokenise; that round trip is the only
thing tying the two ends together, as nothing in this app consumes its own
URIs at runtime.

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
  simpler than mutating one in place. That's what `menu::render` does.
- **`muda::Menu::new` panics off the main thread** — "`muda::Menu` can only be
  created on the main thread". `cargo test` runs each test on a spawned
  thread, so `menu::render` cannot be unit-tested at all, and no amount of
  restructuring will change that. This is *why* `menu.rs` is split the way it
  is: every decision (which sections appear, what each label says, which song
  a jump targets) lives in `menu::model` and `MenuModel`'s `show_*_section`
  methods, which are pure and tested, leaving `render` as untested but
  decision-free item construction. Putting a condition back into `render`
  silently drops it out of test coverage.
- **Don't add `muda` as a direct dependency.** `tray_icon::menu` re-exports it
  wholesale, so nothing needs it in `Cargo.toml` — and declaring it there lets
  Cargo resolve a second muda version independently of tray-icon's. Since
  muda's menu-event handling is process-global, that failure mode is menu
  clicks silently no longer arriving, which is a miserable thing to debug. It
  was a direct dependency until the post-review cleanup; the removal is
  deliberate.

## `PlayerSnapshot` and `PlaylistCache` are separate, independently refreshed

`mpd.rs` holds two caches, not one, because they change for different
reasons and at very different costs:

- `PlayerSnapshot` (status + current song + queue) — the queue is the
  expensive part (2,734 tracks ≈ 2.2 MiB, ~342 ms to fetch), so it's only
  refetched on `Subsystem::Queue`. `Subsystem::Player` and
  `Subsystem::Options` refetch just status + current song and reuse the
  cached queue — see the `Options` gotcha above.
- `PlaylistCache` (song URL → playlist names, plus the raw playlist name
  list) — reloaded in full on `Subsystem::StoredPlaylist`. MPD's event
  doesn't say *which* playlist changed, and a full reload of all 45
  playlists / ~12.4k entries is cheap enough not to bother tracking deltas.

The merge lives in `mpd.rs`'s idle loop (`run_idle_loop`) rather than in
`menu.rs`, because that's the one place already matching on `Subsystem` to
decide what to refetch — `menu.rs` just receives both caches, reduced to
plain primitives, and stays pure (see the `#[non_exhaustive]` note above).
Reloading the playlist cache on `StoredPlaylist` also covers the app's own
`playlistadd` writes, so an "Add to" click needs no manual cache patching —
one mutation path that can't drift out of sync.

## Album traversal: two numbers that look arbitrary but aren't

Both were derived by measuring the real library on 2026-07-27 (28,928 tracks,
1,877 directories, 1,845 distinct album titles, 45 stored playlists). Don't
"tune" either without re-measuring.

**Albums are identified by parent directory, not by tags.** The obvious rule —
group by `(album, albumartist)` — is wrong on this library: it merges
`Peter Gabriel / Peter Gabriel`, which is genuinely *two different self-titled
albums* (`[1984]` and `[2002]`), into one 30-track blob, and does the same to
`The Clash [1999]` / `[2002]`. Directory grouping keeps them apart, needs no
tags at all (4 tracks have no `album` tag, 30 have no `albumartist`), and is
correct on every multi-disc album here, since all of them live in a single
directory with continuous track numbering rather than `CD1/`/`CD2/`
subdirectories. Known cost: 2 directories under `Unnmatched/` hold more than
one album, so they read as a single long album. Accepted — 2 wrong in 1,877.

**Album nav appears only inside a run of 5+ same-directory tracks.** The
threshold separates album listening from track-oriented playlists, measured as
the percentage of queue positions sitting in a same-directory run of at least
5:

| Playlist | tracks | in a run of ≥5 |
|---|---|---|
| rem_all | 210 | 100% |
| hottest_100_best | 86 | 99% |
| mashup_albums | 244 | 97% |
| Beatles_best | 61 | 90% |
| My Top Rated | 2,734 | 18% |
| Five stars | 444 | 5% |
| mellow_2024 | 144 | 4% |

The gap between 90% and 18% is wide enough that anything from 5 to 7 behaves
the same; 4 starts letting shuffle-ish playlists through. Read the rule as
*"am I inside an album block right now"*, not *"is this queue album-ordered"* —
when `My Top Rated` reaches its one 37-track album run, showing album nav is
correct, not a false positive.

`random` also hides album nav, because MPD's random mode plays the queue in a
random *order* while leaving queue order itself untouched — so queue-order
jumps would send you somewhere unrelated to what plays next.

## Menu glyphs are pinned to text presentation

Every menu glyph carries a `U+FE0E` variation selector (`"\u{23EF}\u{FE0E}"`
and friends). Without it macOS renders these codepoints as wide colour emoji,
which look wrong against the menu-bar font weight. Album nav deliberately uses
*doubled* arrows (`⏮⏮`/`⏭⏭`) rather than `⏪`/`⏩`: the fast-forward glyphs
default to colour-emoji presentation, so they'd depend on `U+FE0E` actually
being honoured to look consistent, whereas doubling a glyph that already
renders correctly does not.

The copy rows are the one exception to the `U+FE0E` rule: they use `⧉`
(U+29C9), which has no emoji presentation to suppress. 📋 (U+1F4CB) is the
obvious literal clipboard, but it has no text presentation form in the system
fonts, so `U+FE0E` can't tame it — it would render as a wide colour blob
against everything else.

The doubled `⏮⏮` is used for *both* states of the prev item ("Start of Album"
and "Prev Album: …") on purpose — the icon stays put while only the label
changes, so the row doesn't appear to flicker between two different items as
you move through an album.

Album names in these labels are truncated to 40 chars (159 of 1,845 titles are
longer; the longest is 192), applied to the name alone so the `Next Album: `
prefix is never pushed off.

## Two different icons, don't conflate them

- The **tray title is text-only** (`TrayIconBuilder::with_title`) — the track
  text, `♫`, or `🔇`. There is no tray icon image asset.
- **`assets/icon.icns`** is the *app bundle* icon, used by Finder/Dock. It's
  currently a generated placeholder; swapping the file is all that's needed to
  change it, no config edits.

## Verification performed

Verified manually against real MPD, not by an automated integration harness
(deliberately — see the README's note on scope).

v1:

- Menu actions: play/pause, next, clear.
- Resilience: killed and restarted MPD mid-session, confirming
  🔇 → backoff → automatic recovery with no app restart, and that the log
  content is genuinely useful for debugging.
- URL scheme end-to-end via real `open 'mpdcontrol://...'` links once packaged
  and installed: a native command (`toggle`), a quoted-path recursive `add`
  (the exact example above), and an `mpc`-fallback command (`volume 50`).

v2 (playlist membership, album traversal, icons, paused indicator) was
similarly walked through live against the real library. **That pass earned its
keep — it found two bugs the 70-test unit suite could not**, both needing real
MPD state to reproduce:

- A stored playlist listing the same song twice (`Five stars.m3u` does, for
  Lorde's "Royals") showed that playlist's name twice under "In playlists".
  Fixed by sorting and deduping each song's membership list.
- Toggling `random` mid-album did nothing until `Subsystem::Options` was
  handled — MPD reports `random`/`repeat`/etc. there, not on `Player`.

Both are the same shape: an assumption about MPD's real-world behaviour that
looked obviously true and wasn't. Budget for a live pass on anything new that
touches MPD state.

v3 (the current track/album copy rows) was walked through live against the
real library, with both the original `pbcopy` implementation and the `arboard`
one that replaced it: clicking each row put the URI on the clipboard, and it
pasted correctly. Better than that, the copied URIs were then opened for real
and came back through Launch Services intact —

```
dispatching mpdcontrol URL command: "add \"Albums/S/Skyhooks/Living in the 70's\""
dispatching mpdcontrol URL command: "add \"Albums/K/Kraftwerk/Autobahn\""
```

— which exercises the whole loop (generate → percent-encode → clipboard →
macOS → parse → percent-decode → `shlex`) on a real path containing an
apostrophe, not just the unit test's synthetic one.

That pass also surfaced one thing that is **not fixed**: if the app isn't
already running when an `mpdcontrol://` link is clicked, Launch Services
starts it and the URL can reach the event loop before the first
`MpdEvent::Connected` does, so `spawn_command` finds no client and logs
`ignoring url command: not connected to MPD`. The command is silently
dropped and the click has to be repeated. It predates the copy rows, but they
make it far easier to hit, since pasting links into notes is exactly the
"app might not be running" case. The fix would be to hold URLs that arrive
while `current_client` is `None` and drain them on the next `Connected`.

`tray-icon`'s known cosmetic quirks (icon rendering, an occasional double icon
on some macOS versions) were not encountered on the macOS version in use.
