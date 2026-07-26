# Implementation plan: playlist membership, album traversal, unicode icons

Source spec: `spec.md`. This plan sequences the work by code layer (config →
mpd.rs → menu.rs → main.rs → command.rs → docs), landing both features
together at each layer, per `AGENTS.md`'s personal-tool-sized scope: small
steps, pure logic kept unit-testable, no speculative abstraction.

Progress is tracked via the checkboxes below. No fixed commit-per-step rule —
commit at your discretion. A consolidated manual QA pass against the live MPD
server closes out the work (§ at the end of this file).

**Status: steps 1–10 done. Next: step 11 (documentation).**

## 0. Technical context and decisions

Gaps the spec leaves to implementation judgment. Only the entries still
relevant to *unstarted* work (steps 8–12) are kept in detail; things steps
1–7 already settled are covered by reading the code, or by `spec.md` where
they amount to a spec clarification.

### 0.1 `PlaylistCache` shape (as built, not as originally sketched)

```rust
#[derive(Debug, Clone, Default)]
pub struct PlaylistCache {
    pub membership: HashMap<String, Vec<String>>, // song URL -> sorted playlist names
    pub names: Vec<String>,                        // every playlist name MPD reported
}
```

`names` was added during step 6 (amending step 3) so `key_playlists` entries
that don't match a real MPD playlist can be validated — `membership`'s keys
alone can't tell "doesn't exist" apart from "exists but is empty".
**`load_playlist_cache` already warns on an unresolvable name** (step 3/6),
but nothing yet *filters* `key_playlists` before it reaches `menu::model()`
— that filtering against `PlaylistCache::names` is still open, in step 10.

### 0.2 `playlistadd` **is** typed — no hand-rolling needed (relevant to step 9)

Confirmed: `mpd_client` exposes `commands::AddToPlaylist::new(playlist,
song_url)`. `command::add_to_playlist` (not yet written) should be a thin
wrapper around it, same shape as `command::next`/`command::clear`. (By
contrast, `listplaylist` *did* need hand-rolling — see `ListPlaylist` in
`mpd.rs`, already implemented, mirroring `command::AddRecursive`.)

### 0.3 Primitive extraction happens at the call site, not in `menu.rs`

`menu::model()` takes only primitives (no `mpd_client` types), per
`AGENTS.md`. As of step 7, `main.rs` already does this extraction in the
`MpdEvent::Connected` arm: `queue_entries()` maps `snapshot.queue` to
`Vec<menu::QueueEntry>`, `current_pos` comes from `Status::current_song`,
`playlists_for_url` comes from the cache. Step 10 should follow the same
pattern for anything dispatch needs (e.g. the current song's URL).

### 0.4 Menu item id format and where each side lives

- Building: `menu::render()` (done, step 7) formats `album:<song_id>` and
  `addto:<name>` via private helpers `album_jump_item_id`/`addto_item_id`.
- Parsing: **not yet written** — belongs in `main.rs`'s `MenuEvent` dispatch
  if-chain (step 10), using `strip_prefix("album:")` /
  `strip_prefix("addto:")` (not `split_once`, despite an earlier spec draft
  — `strip_prefix` only strips the fixed literal prefix, so it can't be
  tripped up by a colon inside the playlist name itself; `spec.md` §8.2 now
  reflects this).
- The round-trip tests for the id *format* already exist, in `menu.rs`
  (`addto_item_id_round_trips_name_containing_a_colon`,
  `album_jump_item_id_round_trips_song_id`) — they test the string contract
  directly, without needing live dispatch code. Step 10 doesn't need to
  duplicate these; it just needs the parsing side to honor the same format.

---

## Steps 1–7 (done)

Summarized here only for facts later steps depend on — see git history /
`spec.md` for full rationale.

1. **`config.rs`** — `favourites` removed, replaced by `key_playlists`
   (`Vec<String>`, default empty). Old config files with a stray
   `favourites` key still load fine (serde ignores unknown keys).

2. **`mpd.rs` — queue cache** — `PlayerSnapshot` carries
   `queue: Vec<SongInQueue>`. Refetched on connect and `Subsystem::Queue`;
   reused as-is on `Subsystem::Player` (which never changes queue contents).

3. **`mpd.rs` — playlist cache** — `PlaylistCache` (shape above) loaded on
   connect and reloaded in full on `Subsystem::StoredPlaylist`; reused
   unchanged on `Subsystem::Queue`/`Subsystem::Player`. A load failure warns
   and falls back to an empty cache rather than propagating. `MpdEvent::Connected`
   is now `(Box<PlayerSnapshot>, Box<PlaylistCache>, Client)`.

4–6. **`menu.rs` — pure model** — all pure, all unit-tested without a live
   MPD connection:
   - `album_key`/`same_album`/`album_run`: directory-based album identity
     and contiguous-run detection.
   - `show_album_nav`, `album_nav` (the latter also folds in the
     "state must be Playing/Paused" gate that isn't `show_album_nav`'s own
     job).
   - `AlbumJump { label, song_id }` — **`label` is the full row text**
     (e.g. `"Prev Album: Abbey Road"`, or literally `"Start of Album"`), not
     just the album name; only the name substring inside it is truncated to
     40 chars. This is what lets `render()` stay a pure "prepend the icon"
     step with no knowledge of which case produced the jump.
   - `next_album_jump`/`prev_album_jump`, `album_display_name` (album tag →
     directory basename → raw URL fallback, then truncate).
   - `in_playlists_list` (case-insensitive sort), `add_to_list` (order-
     preserving exclusion). **`add_to_list` does not itself validate playlist
     existence** — it trusts whatever `key_playlists` slice it's given. The
     omission of an unresolvable name from "Add to" (spec §3/§9) only
     happens if the *caller* filters `key_playlists` against
     `PlaylistCache::names` first — still step 10's job (see §0.1 above).
   - `MenuModel { full_text, prev_album, next_album, in_playlists, add_to }`
     and `model(...)` assembling them, per spec §8.1. **No `title` field** —
     an earlier spec draft had one, but nothing consumes it (the tray title
     is set independently via `Display`); cut per YAGNI, documented in
     `spec.md`.

7. **`menu.rs` — render + icons; `main.rs` wiring** — `render(&MenuModel)`
   turns the model into `muda` items: icons (all `U+FE0E`-suffixed for
   monochrome/narrow presentation), the optional album-nav block, the
   existing disabled full-title row, "In playlists"/"Add to" submenus, and
   `Quit`. Separators are inserted only immediately before a block that
   turns out non-empty, so two never appear adjacent.

   `main.rs`'s `MpdEvent::Connected` arm was fully rewired to call
   `menu::model(...)` + `menu::render(...)` with **real** data — not a
   placeholder — including the queue, current position, and playlist
   membership for the current song. `key_playlists` is captured (cloned)
   before `config` moves into `mpd::run`. **Not yet added:** any click
   dispatch for the new `album:`/`addto:` ids (still just falls through
   the existing if-chain unrecognized), and no cross-event held state
   exists yet for dispatch to use — that's step 10.

**Verification as of step 7:** `cargo build`/`clippy --all-targets` clean
except two expected-transient `dead_code` warnings (`PAUSED_ICON`, resolves
step 8; `PlaylistCache::names`, resolves step 10). `cargo test`: 66/66
passing.

---

## 8. `menu.rs` — paused indicator

Separate from the `MenuModel` work — this only touches the tray-title
`Display` path (`track_display` / `display_for_snapshot`), not the dropdown.

- [x] Add the paused glyph constant (spec §7): `PAUSED_ICON` already exists
      in `menu.rs` (added in step 7, currently unused) — use it here.
- [x] Prefix `⏸ ` outside the existing 25-char truncation budget, only when
      `status.state == PlayState::Paused`. Implemented by widening
      `track_display`'s signature to take a `state: PlayState` param (asked
      the user; confirmed over keeping `track_display` state-free with a
      separate wrapper, or inlining the prefix only in
      `display_for_snapshot`) — prefixes `truncated`, not `full`, so the
      dropdown's untruncated `full_text` never carries the glyph.
- [x] `Stopped` and `Playing` unchanged.

Tests (spec §9 "Paused indicator"):
- [x] `Paused` prefixes the glyph; `Playing`/`Stopped` do not
- [x] the prefix doesn't consume the 25-char budget (a title that exactly
      fills 25 chars truncates identically paused vs. playing)
- [x] a long title truncates to the same text whether paused or playing
      (only the prefix differs)

**Verify:** `cargo test menu::`. Manual QA: pause playback, confirm `⏸`
appears in the tray and the track text is unaffected.

## 9. `command.rs` — new command functions

- [x] `play_song(client: &Client, id: SongId) -> Result<()>` wrapping
      `Play::song(id)` (confirmed typed support exists — `Play::song<S: Into<Song>>`,
      and `SongId` implements `Into<Song>`).
- [x] `add_to_playlist(client: &Client, playlist: &str, url: &str) -> Result<()>`
      wrapping `AddToPlaylist::new(playlist, url)` (§0.2 above — typed, no
      hand-rolling).
- [x] No changes to `ParsedCommand`, `dispatch`, or `dispatch_or_fallback` —
      spec §8.4 keeps these out of scope; album nav and playlist-add stay
      menu-only.

**Verify:** These are thin wrappers with no branching logic — no new unit
tests needed beyond what already exists for `next`/`clear`'s shape. Verified
functionally in step 10's manual QA (clicking the menu items that call them).

## 10. `main.rs` — state and dispatch

Note: unlike the original draft of this step, **menu construction is
already wired** (step 7) — `queue_entries()`, `current_pos`,
`playlists_for_url`, and the `menu::model()`/`menu::render()` call site all
exist and work today. What's left is specifically about *dispatch*:

- [x] Widen held state: added `current_url: Option<String>` alongside
      `play_state`/`current_client`, updated on `Connected`/`Disconnected`.
      **Narrower than the original draft above** — asked the user; decided
      *not* to also hold the last `PlayerSnapshot`/`PlaylistCache`, since
      nothing in dispatch actually reads them: `album:<id>` only needs to
      parse the id (the id already names the target `SongId` directly, no
      queue lookup needed — that's the whole point of using `SongId` over
      `SongPosition`, spec §5.4), and `addto:<name>` only needs the current
      song's URL. Holding the full snapshot/cache would be unread state,
      against `AGENTS.md`'s no-speculative-complexity stance.
- [x] Filter `key_playlists` against `PlaylistCache::names` before calling
      `menu::model(...)` (spec §3 / §0.1 above) — `known_key_playlists` is
      computed in the `Connected` arm and passed instead of the raw
      `key_playlists` config list. The `warn` on an unresolvable name already
      happens in `mpd.rs::load_playlist_cache`; this only adds the filtering.
- [x] Add the two dispatch branches (spec §8.3), using `command::play_song`/
      `command::add_to_playlist` (step 9). Malformed `album:` ids (parse
      failure) are logged and dropped, not panicked on; an `addto:` click
      with no `current_url` (nothing playing) is likewise logged and
      dropped.

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
      to "Checking crate APIs", and the `listplaylist` hand-roll (already
      implemented in `mpd.rs`) to the same section (parallel to the existing
      `AddRecursive` note).
- [ ] `docs/implementation-details.md` — optionally add a short note on the
      `PlayerSnapshot`/`PlaylistCache` split and why the merge lives in
      `mpd.rs`, matching the file's existing "notes for humans" style. Not
      required by the spec but keeps the file's stated purpose intact.

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
