# Agent notes

A Cargo workspace with three crates:

- `crates/controller` — a macOS menu-bar MPD controller plus an `mpdcontrol://`
  URL scheme. Complete and in daily use: playback controls, album traversal,
  playlist membership and "Add to", copy-URI rows for the current track and
  album, plus the URL scheme.
- `crates/albumdb` — the SQLite album database: schema, domain types, and all
  the pure derivation logic.
- `crates/album-tool` — the `scan`/`sync`/`status` CLI that maintains it.
- `crates/mpd-conf` — parses MPD's own `mpd.conf`. Both binaries infer the MPD
  port, and the album database's location, from it rather than being told
  twice. Returns `music_directory` as an `Option`: `album-tool` requires it,
  the menu-bar app doesn't.

See `README.md` for architecture and usage, `docs/implementation-details.md`
for the non-obvious details behind the controller — including the measured
evidence behind the album-grouping and run-length rules, which should not be
re-derived or "tuned" without re-measuring — and `docs/album-database.md` for
the same for the album database.

Scope is deliberately personal-tool-sized: prefer simple working code over
exhaustive coverage, and don't design in speculative complexity. Ideas that
were explicitly deferred are in `docs/future_expansions.md` — check there
before proposing "obvious" additions.

## Don't cite `spec.md` / `plan.md` from code

Those are **working documents, deleted when an epic completes.** It has
happened twice already, and `controller/src/menu.rs` and `mpd.rs` carried
dangling "spec §7.1" / "spec §3" references to long-dead specs for months
because of it.

A comment must stand on its own. Either state the fact inline, or cite a file
under `docs/` — never a section number in a document that won't outlive the
branch.

## Quality gates

No CI in this repo — run all three by hand before calling any work done:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Default rustfmt/clippy settings; no config file for either, deliberately.

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
- `mpd_client` 1.4.1's `Tag` enum has a typed `OriginalDate` variant but **no**
  variant for `MUSICBRAINZ_RELEASEGROUPID` (or any other unrecognized tag) —
  those fall into the catch-all `Tag::Other(Box<str>)`, and `SongBuilder`
  applies this to every field key, so nothing sent by MPD is ever dropped.
  Confirmed live via `CurrentSong`: `OriginalDate` and
  `Other("MUSICBRAINZ_RELEASEGROUPID")` both came through correctly on the
  same song. No need to special-case tags the enum doesn't know about.
- `rusqlite` 0.40.1's `bundled` feature compiles `libsqlite3-sys` 0.38.1,
  whose build script uses `cfg_select!` — stable only on Rust ~1.97+. On an
  older stable toolchain (seen: 1.92.0) the build fails with "use of unstable
  library feature `cfg_select`". Fix: `rustup update`. Not a code issue.
- `musicbrainz_rs` 0.13.0 needs the `async` feature enabled (off by default)
  for `execute_async`/`execute_with_client_async` to exist at all. There's no
  free `set_user_agent()` function — construct
  `MusicBrainzClient::new(user_agent)` and pass it explicitly to
  `execute_with_client_async`. `ReleaseGroup::fetch().with_genres()` populates
  `genres: Option<Vec<Genre>>` with vote counts on `Genre.count`, and
  `.with_releases()` populates `releases: Option<Vec<Release>>` — both
  confirmed live, matching the genre counts in `docs/album-database.md`
  exactly.
- **`musicbrainz_rs` 0.13.0's built-in rate limiter is unreachable from the
  outside — don't rely on it.** Its `rate_limit` feature enables `dep:governor`
  but never actually uses it (confirmed: no `governor` reference anywhere in
  the crate's source), and never forwards to `api_bindium`'s own `rate_limit`
  feature — the thing that actually gates `client.await_rate_limit()` inside
  `try_send_request_async`. So there is no way, through any combination of
  public feature flags, to turn on real MusicBrainz-side rate limiting.
  `album-tool/src/musicbrainz.rs`'s `RateLimiter` (own `Mutex<Option<Instant>>`
  + `min_interval`) replaces it — resolves the "still to be decided" note this
  entry used to carry. Separately, the crate *does* auto-retry a bare `503`
  internally (unconditionally, not behind a feature — default `max_retries =
  10`, using the response's `Retry-After` header if present else its own
  backoff formula), but **not** other `5xx` statuses — those return a single
  `Err` on first failure, which is why `RealMusicBrainzClient` wraps its own
  outer retry/backoff loop (`backoff_duration`) around the whole call. `404`
  is detectable cleanly via
  `err.as_musicbrainz_error().is_some_and(|e| e.is_not_found())`, without
  needing the raw HTTP status at all.
- `mpd_client` 1.4.1's `filter::Filter` type only supports tag comparisons
  (`Filter::tag`, `Filter::new(tag, operator, value)`) — there is no `base`
  variant, so it can't express `(base '<path>')`. There's also no typed
  command for `find`/`lsinfo`/`listfiles`. `album-tool/src/library.rs` hand-
  rolls all three as `Command` impls (same pattern as `controller`'s
  `AddRecursive`/`ListPlaylist`), and `album-tool/src/mpd_wire.rs` builds the
  `base` filter clause by hand. Its inner escaping (backslash then
  apostrophe) only needs to handle `\`/`'` — the *outer*
  protocol-level escaping (backslash-escaping `'`/`"`/`\`, wrapping in double
  quotes because paths contain spaces) is applied automatically by
  `mpd_protocol::command::escape_argument` once the clause is passed through
  `RawCommand::argument`, so there's nothing to duplicate there.
- `Song::from_frame_multi`/`SongInQueue::from_frame_multi` (the response
  parsers `Find`/`Queue`/etc. use internally) are `pub(crate)` inside
  `mpd_client` — not reachable from outside the crate. Any hand-rolled
  `Command` (see above) has to parse `Frame`'s `(key, value)` pairs itself;
  `mpd_wire::parse_directory_listing` is that parser for this project, shared
  by the real MPD commands and the fixture loader alike.
- **`find`/`lsinfo` send `Last-Modified` as the field immediately after every
  `file:` line, before any tag.** A hand-rolled parser that treats
  `Last-Modified` as a record boundary (as `parse_directory_listing` once
  did, lumped in with `playlist`) will finalise the song right there and
  silently drop every tag that follows — confirmed live: every real scan
  produced 1,886 albums with `path` set and every other field `None`, not
  caught by the committed fixture because it has no `Last-Modified` lines at
  all. `Last-Modified` (like `Added`/`Format`/`Time`/the `*Sort` fields) must
  fall through to `apply_tag`'s catch-all and be ignored, not treated as a
  boundary — only `file`/`directory`/`playlist` actually start a new record.

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
