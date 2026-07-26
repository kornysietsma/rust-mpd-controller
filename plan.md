# Implementation plan: playlist membership, album traversal, unicode icons

Source spec: `spec.md`. This plan sequences the work by code layer (config →
mpd.rs → menu.rs → main.rs → command.rs → docs), landing both features
together at each layer, per `AGENTS.md`'s personal-tool-sized scope: small
steps, pure logic kept unit-testable, no speculative abstraction.

Progress is tracked via the checkboxes below. No fixed commit-per-step rule —
commit at your discretion. A consolidated manual QA pass against the live MPD
server closes out the work (§ at the end of this file).

## 0. Technical context and decisions

These fill gaps the spec leaves to implementation judgment. Flagging them
here so they're visible before code lands, not buried in a diff.

### 0.1 `PlayerSnapshot` carries the queue; a new `PlaylistCache` rides alongside it

```rust
#[derive(Debug, Clone)]
pub struct PlayerSnapshot {
    pub status: Status,
    pub song: Option<Song>,
    pub queue: Vec<SongInQueue>,
}

#[derive(Debug, Clone, Default)]
pub struct PlaylistCache {
    /// song URL -> sorted playlist names containing it.
    pub membership: HashMap<String, Vec<String>>,
}
```

`mpd.rs` keeps both as local state inside `run()`, across the idle loop,
and republishes **both together** on every `MpdEvent::Connected`, per the
existing pattern (the enum already carries a full snapshot each time — this
just widens it):

```rust
pub enum MpdEvent {
    Connected(Box<PlayerSnapshot>, Box<PlaylistCache>, Client),
    Disconnected,
}
```

Rationale: `main.rs` only ever needs "the latest known everything" to build
a menu; it never needs to know which piece changed. Keeping that merge
inside `mpd.rs` (which already tracks it to decide what to refetch) avoids
adding merge logic to `main.rs`. The "reuse cached queue on Player events"
optimization from spec §4.1 is purely about avoiding an MPD round trip — it
doesn't change what gets published.

Loading the playlist cache is a separate async step from fetching the
queue; a connect does both (fresh queue fetch + fresh playlist load) before
the first publish. See step 3's design note below for exactly how this
threads through `run_idle_loop` alongside the queue cache added in step 2.

### 0.2 `listplaylist` needs a hand-rolled command

`mpd_client` 1.4.1 has no typed wrapper for `listplaylist <name>` (untagged
filenames) — only `GetPlaylist` (`listplaylistinfo`, tagged/expensive) and
`GetPlaylists` (`listplaylists`, playlist names only). Confirmed by reading
the vendored source (`~/.cargo/registry/src/.../mpd_client-1.4.1/src/commands/definitions.rs`).

Hand-roll it in `mpd.rs`, next to where it's used (mirrors `AddRecursive` in
`command.rs`, which is the existing precedent for this exact situation):

```rust
struct ListPlaylist<'a>(&'a str);

impl MpdCommand for ListPlaylist<'_> {
    type Response = Vec<String>;
    fn command(&self) -> RawCommand {
        RawCommand::new("listplaylist").argument(self.0)
    }
    fn response(self, frame: Frame) -> Result<Self::Response, TypedResponseError> {
        Ok(frame.into_iter().map(|(_, v)| v).collect())
    }
}
```

Each returned line's key is `file`; values are the song URLs, which is all
`listplaylist` sends.

### 0.3 `playlistadd` **is** typed — no hand-rolling needed

Spec §8.4 asks to check before hand-rolling. Confirmed: `mpd_client` exposes
`commands::AddToPlaylist::new(playlist, song_url)`. `command::add_to_playlist`
is a thin wrapper around it, same shape as `command::next`/`command::clear`.

### 0.4 Primitive extraction happens at the `main.rs` call site

Per spec §8.1's doc comment ("Extracted at the call site") and the existing
`AGENTS.md` rule: `menu::model()` takes only primitives. `main.rs` builds
`Vec<menu::QueueEntry>` from `PlayerSnapshot.queue` (mapping
`SongInQueue.song.url`, `.song.album()`, `.id.0`) immediately before calling
`menu::model(...)`, the same way it already extracts `song.title()` /
`song.artists()` today via `menu::display_for_snapshot`. `display_for_snapshot`
stays as the thin non-pure bridge for the tray title; a parallel small bridge
is added for the menu model's inputs.

### 0.5 Where `album:`/`addto:` id parsing lives

Per spec §8.3, both `split_once(':')` parses live in `main.rs`'s dispatch
if-chain, matching where `PLAY_PAUSE_ID` etc. are already matched. The
round-trip tests for these (spec §9 "Menu ids") live in `main.rs`'s test
module, since that's the only place both the id-building (`render()`) and
id-parsing (dispatch) are in scope together — actually the *building* side
(`format!("addto:{name}")`) lives in `menu::render`, so the round-trip test
constructs an id string directly and asserts the parse side recovers it,
without needing a live `Menu`.

---

## 1. `config.rs` — remove `favourites` — ✅ done

Removed the `favourites` field from `Config`/`Default`; updated the two
existing tests; added `stray_favourites_key_is_ignored` confirming an old
config file with a leftover `favourites` key still loads fine. No other code
referenced the field. `cargo test`/`clippy` clean (33/33 tests).

## 2. `mpd.rs` — queue cache — ✅ done

`PlayerSnapshot` now carries `queue: Vec<SongInQueue>`. Added `fetch_queue`
and `fetch_status_and_song` helpers, and split publishing into:
- `publish_full_snapshot(client, on_event) -> Result<Vec<SongInQueue>>` —
  fetches status+song+queue, publishes, returns the queue for caching. Used
  on connect and on `Subsystem::Queue`.
- `publish_player_snapshot(client, queue: &[SongInQueue], on_event) -> Result<()>`
  — fetches status+song only, publishes with the passed-in (cached) queue.
  Used on `Subsystem::Player`.

`run()` holds `let mut queue = ...` and passes `&mut queue` into
`run_idle_loop`, which updates it only on `Subsystem::Queue` (via the match
arm `Subsystem::Queue => ... *queue = fresh_queue`). Other subsystems fall
through a `_ => {}` arm — step 3 adds `Subsystem::StoredPlaylist` there.

`cargo build`/`clippy`/`test` clean, except one expected transient `dead_code`
warning on `PlayerSnapshot.queue` — nothing reads it yet since `menu.rs`/
`main.rs` aren't wired up until steps 6–10. Not worth silencing; it resolves
itself as later steps land.

## 3. `mpd.rs` — playlist cache

### Design note: threading two caches through `run_idle_loop`

Per spec §4.2 and this plan's §0.1, **every** publish must carry "the latest
known everything" — so `playlist_cache` needs the same reuse-on-other-events
treatment that `queue` already got in step 2, just keyed off a different
subsystem. Concretely, `run_idle_loop` needs a second threaded local,
`playlist_cache: &mut PlaylistCache`, alongside `queue`:

| Trigger | status+song | queue | playlist_cache |
|---|---|---|---|
| Connect | fetch | fetch | fetch |
| `Subsystem::Queue` | fetch | fetch | **reuse cached** |
| `Subsystem::Player` | fetch | reuse cached | **reuse cached** |
| `Subsystem::StoredPlaylist` | fetch | reuse cached | **fetch** |

Status+song are cheap enough that every path just refetches them (matching
the existing `publish_player_snapshot` shape) — only `queue` and
`playlist_cache` are expensive enough to cache and reuse, each independently
keyed to the one subsystem event that actually changes it.

Concretely this means: `publish_full_snapshot` and `publish_player_snapshot`
both gain a `playlist_cache: &PlaylistCache` parameter (to embed in the
published event); a new `publish_playlist_snapshot(client, queue: &[SongInQueue], on_event) -> Result<PlaylistCache>`
handles the `StoredPlaylist` branch (fetch status+song, reload playlist
cache, reuse queue, return the fresh cache for `run_idle_loop` to store).
`publish_full_snapshot`'s `Subsystem::Queue` branch reuses the cached
`playlist_cache` unchanged.

- [x] Add the hand-rolled `ListPlaylist` command (§0.2).
- [x] Add `PlaylistCache` struct (§0.1) and a
      `load_playlist_cache(client) -> Result<PlaylistCache>` helper:
      `listplaylists` for names, then `listplaylist <name>` per name, folding
      into the `url -> Vec<name>` map (sorted names per entry).
      Do **not** deduplicate `listplaylists` results — `mpd_client` (unlike
      `mpc`) returns the clean 45-name list already (spec §2.1).
- [x] Load on connect (alongside the queue fetch).
- [x] Reload (full, all playlists) on `Subsystem::StoredPlaylist` — add a
      `Subsystem::StoredPlaylist` match arm in `run_idle_loop` (currently
      falls through the `_ => {}` catch-all left there by step 2).
- [x] On a load failure, `warn!` and publish an empty `PlaylistCache` rather
      than propagating the error (spec §4.2 — menu renders with empty
      membership, not a failure state).
- [x] Widen `MpdEvent::Connected` to `(Box<PlayerSnapshot>, Box<PlaylistCache>, Client)`.

Implemented per the design note above: `publish_full_snapshot` and
`publish_player_snapshot` both gained a `playlist_cache: &PlaylistCache`
parameter that they embed unchanged; the new `publish_playlist_snapshot`
handles the `StoredPlaylist` branch and returns the freshly loaded cache for
`run_idle_loop` to store. `load_playlist_cache_or_warn` centralizes the
warn-and-default-on-failure behaviour so both the initial connect and the
`StoredPlaylist` refresh share it. `main.rs`'s `MpdEvent::Connected` match arm
was updated to destructure the new third field as `_playlists` (unused until
step 10), matching step 2's precedent of leaving now-plumbed data unread
until the layer that consumes it lands.

**Verify:**
- `cargo build` + `cargo clippy --all-targets` clean, except the same
  transient `dead_code` warning pattern as step 2 — now also covering
  `PlaylistCache.membership` — which resolves once steps 6/10 read it.
- `cargo test` clean (33/33 — no new tests added this step; the pure logic
  this step touches is exercised by manual QA in step 12, per the plan's
  existing note that these are thin wiring/fetch functions).
- Deferred to manual QA: confirm playlist cache reload count/timing in logs
  after a `mpc addplaylist` from outside the app, and after the app's own
  "Add to" click (spec §6.2's "implicit feedback" claim — one write path).

## 4. `menu.rs` — pure model, part 1: album grouping and run detection

New module-private (or `pub(crate)`) functions, unit-tested without any
`mpd_client` types per spec §9.

- [x] `album_key(url: &str) -> Option<&str>` — parent directory of a song
      URL; `None` for a root-level file (spec §5.1).
  - [x] test: same directory → same album
  - [x] test: different directory → different album
  - [x] test: multi-disc (same dir, non-contiguous-looking track numbers) →
        one album
  - [x] test: root-level file (no `/`) → `None`, no panic
- [x] Run detection: given `queue: &[QueueEntry]` and `current_pos: usize`,
      compute `(run_start, run_end)` — the maximal contiguous same-`album_key`
      range containing `current_pos`. Scan outward from `current_pos` only.
  - [x] test: run length at start / middle / end of a run
  - [x] test: single-track run → length 1
  - [x] test: empty queue → no panic, no run
  - [x] test: queue of exactly one track
  - [x] test: `current_pos` out of bounds → no nav, no panic

`QueueEntry` (spec §8.1's full 3-field shape — `url`, `album`, `id`) was
defined here already, one step ahead of its step-5 checkbox, since
`album_run`'s signature needs it now; only `url` is read so far, so `album`/
`id` sit as expected transient dead fields until step 5 reads them for the
label and jump target. Added a private `same_album` helper so a library-root
file (no parent directory) never matches another root-level file as "same
album" — `album_key` returning `None` for both isn't treated as equal.

**Verify:** `cargo test menu::` — fully covered by unit tests (10 new tests),
no live connection needed. `cargo build`/`clippy --all-targets` clean apart
from the expected transient `dead_code` warnings (now also covering
`QueueEntry`, `album_key`, `same_album`, `album_run`, alongside the
pre-existing `mpd.rs` ones) — these resolve as step 5 wires `model()` up to
call them. `cargo test` full suite: 43/43 passing.

## 5. `menu.rs` — pure model, part 2: visibility and traversal

- [ ] `show_album_nav(random: bool, run_len: usize) -> bool` implementing
      spec §5.3 (`!random && run_len >= 5`).
  - [ ] test: run of 5 → shown; run of 4 → hidden
  - [ ] test: `random = true` → hidden regardless of run length
  - [ ] test: stopped / no current song → hidden (via the caller passing
        `None` for `current`, not inside this function)
- [ ] `QueueEntry` and `AlbumJump` structs (spec §8.1).
- [ ] Traversal functions implementing spec §5.4 — next-album target, and
      the two-state prev/start-of-album target. Both share the `AlbumJump`
      shape; keep them as one function returning `Option<AlbumJump>` per
      direction so `model()` stays a straight assembly of the two calls.
  - [ ] test: next album returns the run's first entry
  - [ ] test: on track 1, prev returns the preceding run's first entry,
        labelled `Prev Album: <name>`
  - [ ] test: on track 2, prev returns the current run's first entry,
        labelled `Start of Album`
  - [ ] test: on the last track of a run, prev still returns the current
        run's first entry
  - [ ] test: two clicks from track 5 of an album — first call lands on
        track 1 (`Start of Album`), a second call (now with `current_pos`
        pointing at track 1) lands on the previous album
  - [ ] test: track 1 of the first album → no prev item
  - [ ] test: track 2+ of the first album → `Start of Album` still offered
  - [ ] test: last album → no next item
  - [ ] test: `repeat` set → still no wraparound (traversal ignores
        `repeat` entirely; test pins that it's not consulted)
- [ ] Album label: name from the target run's first track's `album` tag,
      falling back to the directory basename; `truncate(name, 40)` applied to
      the name only, not the `Next Album: ` prefix (spec §5.2).
  - [ ] test: name taken from the target run's first track
  - [ ] test: falls back to directory basename when `album` tag absent
  - [ ] test: 192-char title truncates to 40 chars without panicking

**Verify:** `cargo test menu::`. This is where the bulk of spec §9's
"Traversal targets" and "Album labels" sections land — exhaustively unit
tested, no live MPD needed.

## 6. `menu.rs` — pure model, part 3: playlist membership and `model()` assembly

- [ ] "In playlists": sort `playlists_for_url` case-insensitively.
  - [ ] test: sorted case-insensitively (mixed-case input)
- [ ] "Add to": `key_playlists.iter().filter(not already in playlists_for_url)`,
      preserving config order.
  - [ ] test: excludes playlists already containing the song
  - [ ] test: empty `key_playlists` → empty result (caller omits the
        submenu — see §6.2's "submenu omitted" is a `render()`-level
        decision, driven by an empty `Vec` here)
  - [ ] test: song in no playlists → "in playlists" empty, "add to" is the
        full (order-preserved) `key_playlists` list
- [ ] Note: "`key_playlists` naming a nonexistent playlist → omitted" (spec
      §9) is a property of how the cache is built (§3, `PlaylistCache` only
      contains playlists MPD actually returned) plus a `warn!` log on cache
      load for names that don't resolve — not logic inside `model()` itself.
      Cross-reference: add the `warn!` in step 3, add a menu-level regression
      test here that a `key_playlists` entry absent from `playlists_for_url`'s
      *universe* still produces a correct (shorter) "Add to" list.
- [ ] Assemble `MenuModel` struct and the `model()` function per spec §8.1's
      signature, wiring together steps 4–6's pieces plus `show_album_nav`
      gating whether `prev_album`/`next_album` are populated at all.
  - [ ] test: `model()` end-to-end with a representative queue/playlist
        fixture — smoke test that the assembly wiring (not just each part in
        isolation) produces the expected `MenuModel`.

**Verify:** `cargo test menu::`.

## 7. `menu.rs` — render, icons, and menu structure

- [ ] Add the icon constants from spec §7 (`U+FE0E`-suffixed literals).
- [ ] Split `build_menu` into `model()` (done, steps 4–6) and `render(model:
      &MenuModel) -> Result<Menu>` per spec §8.1. `render` makes no decisions:
      it turns already-computed `MenuModel` fields into `muda` items,
      including:
  - Play/Pause, Next, Clear (icons added, ids unchanged)
  - optional album-nav section (0, 1, or 2 rows), only when either jump is
    `Some`
  - the existing disabled full-title item (unchanged logic)
  - "In playlists" submenu (disabled label items, spec §6.1), omitted when
    empty
  - "Add to" submenu (enabled items, id `addto:<name>`, spec §6.2), omitted
    when empty or `key_playlists` is empty
  - Quit
  - separators only between non-empty sections (spec §6: "never shows two
    adjacent separators")
- [ ] Update `main.rs`'s call site from `menu::build_menu(...)` to
      `menu::model(...)` + `menu::render(...)`.
- [ ] Menu id round-trip tests (§0.5): `addto:` with a playlist name
      containing a colon; `album:<song_id>` parses back to the right id.
      These test the string format, not a live `Menu` object.

**Verify:**
- `cargo test menu::` for the id round-trip tests.
- `cargo build` clean (muda item construction can't be asserted on beyond
  compiling — `AGENTS.md`'s existing constraint).
- Deferred to manual QA: visual check of separators-never-doubled across the
  track-1 / track-2+ / no-album-nav / no-playlists menu states, and icon
  glyph rendering (spec's explicit "verify visually once running" callout —
  don't screenshot, just report what's shown, per `AGENTS.md`).

## 8. `menu.rs` — paused indicator

Separate from the `MenuModel` work — this only touches the tray-title
`Display` path (`track_display` / `display_for_snapshot`), not the dropdown.

- [ ] Add the paused glyph constant (spec §7).
- [ ] Prefix `⏸ ` outside the existing 25-char truncation budget, only when
      `status.state == PlayState::Paused`.
- [ ] `Stopped` and `Playing` unchanged.

Tests (spec §9 "Paused indicator"):
- [ ] `Paused` prefixes the glyph; `Playing`/`Stopped` do not
- [ ] the prefix doesn't consume the 25-char budget (a title that exactly
      fills 25 chars truncates identically paused vs. playing)
- [ ] a long title truncates to the same text whether paused or playing
      (only the prefix differs)

**Verify:** `cargo test menu::`. Manual QA: pause playback, confirm `⏸`
appears in the tray and the track text is unaffected.

## 9. `command.rs` — new command functions

- [ ] `play_song(client: &Client, id: SongId) -> Result<()>` wrapping
      `Play::song(id)` (confirmed typed support exists — `Play::song<S: Into<Song>>`,
      and `SongId` implements `Into<Song>`).
- [ ] `add_to_playlist(client: &Client, playlist: &str, url: &str) -> Result<()>`
      wrapping `AddToPlaylist::new(playlist, url)` (§0.3 — typed, no
      hand-rolling).
- [ ] No changes to `ParsedCommand`, `dispatch`, or `dispatch_or_fallback` —
      spec §8.4 keeps these out of scope; album nav and playlist-add stay
      menu-only.

**Verify:** These are thin wrappers with no branching logic — no new unit
tests needed beyond what already exists for `next`/`clear`'s shape. Verified
functionally in step 10's manual QA (clicking the menu items that call them).

## 10. `main.rs` — state and dispatch

- [ ] Widen held state: alongside `play_state`, keep the last
      `PlayerSnapshot` (for its queue) and last `PlaylistCache`, plus the
      current song's URL (needed for `add_to_playlist` dispatch).
- [ ] On `MpdEvent::Connected(snapshot, playlists, client)`: extract
      `QueueEntry` vec (§0.4), call `menu::model(...)`, then `menu::render(...)`,
      set tray title via the paused-aware `Display` (step 8) and set the menu.
- [ ] Add the two dispatch branches (spec §8.3):
  ```rust
  } else if let Some(rest) = menu_event.id.0.strip_prefix("album:") {
      // parse rest as u64, spawn_command(... command::play_song(&c, SongId(id)) ...)
  } else if let Some(name) = menu_event.id.0.strip_prefix("addto:") {
      // spawn_command(... command::add_to_playlist(&c, name, &current_url) ...)
  }
  ```
  Malformed `album:` ids (parse failure) are logged and dropped, not
  panicked on.
- [ ] Menu id round-trip tests (§0.5) live here, next to the dispatch parsing.

**Verify:**
- `cargo test` (full suite) + `cargo clippy` clean.
- Manual QA (step 12) exercises this end-to-end — it's the layer that wires
  everything else together, so it's best verified live rather than mocked.

## 11. Documentation

- [ ] `README.md` — document the new menu items (album nav, In playlists, Add
      to), the `favourites` → `key_playlists` config change, icon glyphs.
- [ ] `docs/future_expansions.md` — remove "Playlist membership + Add to",
      "Favourites action", "Album traversal", "In-memory library cache" from
      the deferred list.
- [ ] `AGENTS.md` — add the `mpc lsplaylists` duplication gotcha (spec §2.1)
      to "Checking crate APIs", and the `listplaylist` hand-roll (§0.2 above)
      to the same section (parallel to the existing `AddRecursive` note).
- [ ] `docs/implementation-details.md` — optionally add a short note on the
      `PlayerSnapshot`/`PlaylistCache` split and why the merge lives in
      `mpd.rs` (§0.1), matching the file's existing "notes for humans" style.
      Not required by the spec but keeps the file's stated purpose intact.

**Verify:** Read-through only; no automated check.

---

## 12. Final manual QA (live MPD)

Run once all steps above are complete and `cargo test && cargo clippy` are
clean. Mirrors the "Verification performed" section in
`docs/implementation-details.md` for v1.

**Setup**
- [ ] Build and install per `install.sh` (URL scheme and Dock-icon behaviour
      need the installed app, not `cargo run` — existing constraint, unrelated
      to this feature set but worth a sanity check since `main.rs` changed).
- [ ] Populate `~/.config/mpd-controller/config.toml` with a real
      `key_playlists` list.

**Playlist membership**
- [ ] Play a song known to be in 2+ playlists → "In playlists" lists all of
      them, disabled, sorted case-insensitively.
- [ ] Play a song in none of `key_playlists` → "Add to" offers all of them,
      in config order.
- [ ] Click an "Add to" entry → song is added in MPD (confirm via `mpc`),
      and the menu updates on next open to move that playlist from "Add to"
      to "In playlists" with **no explicit refresh** (spec §6.2's implicit
      feedback claim).
- [ ] Play a song in every configured `key_playlists` entry → "Add to" is
      omitted entirely.
- [ ] Play a song in zero playlists → "In playlists" is omitted entirely.
- [ ] Set `key_playlists` to include a name that doesn't exist in MPD →
      confirm a `warn` in the log, and it's silently absent from "Add to".

**Album traversal**
- [ ] Play into a same-directory run of 5+ tracks → album nav rows appear.
- [ ] Play a track-oriented playlist (run < 5) → album nav is hidden.
- [ ] Enable `random` mid-album-run → album nav disappears; disable → it
      reappears (if still in a run ≥ 5).
- [ ] Track 1 of an album → "Prev Album: <preceding name>" shown.
- [ ] Track 2+ of an album → "Start of Album" shown (no album name in the
      label).
- [ ] Click "Next Album" → lands on the first track of the following run,
      label matches the album actually landed on.
- [ ] Click "Start of Album" then click "Prev Album" again → two clicks
      reach the previous album, per spec §5.4.
- [ ] At the last album in the queue → "Next Album" is absent.
- [ ] At track 1 of the first album → "Prev Album"/"Start of Album" row is
      absent entirely; at track 2+ of the first album, "Start of Album" is
      still present.
- [ ] Confirm no wraparound with `repeat` on.
- [ ] Confirm the 40-char truncation on a genuinely long album title (library
      has titles up to 192 chars) doesn't push the `⏭⏭ Next Album: ` prefix
      off-menu.

**Icons and paused indicator**
- [ ] Report what's actually displayed for each glyph (Play/Pause, Next,
      Clear, Prev/Start-of-Album, Next Album, paused tray icon) — per
      `AGENTS.md`, describe rather than screenshot.
- [ ] Confirm glyphs render monochrome/narrow, not colour emoji.
- [ ] Pause playback → `⏸ ` prefix appears on the tray title; resume →
      it disappears; stop → track text (if any) shows with no prefix.

**Resilience (regression check — queue/playlist caching is new machinery)**
- [ ] Kill and restart MPD mid-session → confirm reconnect still works and
      the menu rebuilds correctly (queue and playlist caches repopulate from
      scratch on the fresh connect, not left stale).
- [ ] With `RUST_LOG=debug`, confirm a `Subsystem::Player`-only event (e.g.
      plain pause/resume) does **not** trigger a `playlistinfo` refetch, and
      a `Subsystem::Queue` event does.
- [ ] Confirm a `StoredPlaylist` event (add/remove elsewhere, e.g. via
      `mpc`) reloads the playlist cache and the menu reflects it next open.

**Existing v1 behaviour (no regressions)**
- [ ] Play/pause/next/clear still work.
- [ ] Existing `mpdcontrol://` URL-scheme commands (`toggle`, quoted-path
      `add`, an `mpc`-fallback command) still work — `command.rs`'s
      `dispatch`/`dispatch_or_fallback` are untouched, but `main.rs`'s state
      handling changed enough to warrant a re-check.
