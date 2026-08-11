# Plan: album data in the menu-bar app

Implements `spec.md` (read that first — every design choice below is decided
there, including the rejected alternatives; this file is only the "in what
order, verified how" breakdown). Both files are working documents — delete
them once this lands, per `AGENTS.md`'s note on stale spec references.

## Technical context

- Four steps, bottom-up: `albumdb` crate → controller worker thread (folding
  in the `config.rs` restructure it needs) → `menu.rs` pure model → `main.rs`
  wiring, with manual QA as that last step's verification. Docs are a fifth,
  final step.
- Every step ends with the repo's standing quality gate (`AGENTS.md`):
  ```sh
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```
- No GUI test harness exists or is being added (`muda::Menu::new` panics off
  the main thread). Pure logic is unit-tested as it's written; the worker
  thread, `render()`, and the `data_version` poll are verified by running the
  app, per the manual checklist in step 4.
- Threads talk over `std::sync::mpsc` (already a transitive dependency via
  `std`, nothing new to add to `Cargo.toml`) — a channel is explicit about the
  serialisation that an `Arc<Mutex<Connection>>` would leave implicit (spec
  §6.1).

---

## Step 1 — `crates/albumdb`: write API for the menu's edits — ✅ done

Landed exactly as specced (`spec.md` §7.1): `open_existing`, the upgraded
`apply_pragmas` journal-mode check, `AlbumFlag`, `set_flag`, `set_rating`
clearing `needs_relisten`, and the `lib.rs`/`album-tool` call-site updates.
Full test list from the plan is in place (see `git log` / `crates/albumdb/src`
for the actual tests — names aren't reproduced here since the code is the
source of truth now).

**One implementation deviation, folded into `spec.md` §7.1**: the upgraded
`journal_mode` assertion accepts `"memory"` as well as `"delete"` —
`open_in_memory()` (used throughout both crates' test suites) can never
report `"delete"`, so a strict assertion broke every in-memory test.

**Verification:** `cargo test -p albumdb` / `-p album-tool` and the full
workspace quality gate (fmt/clippy/test) all green.

---

## Step 2 — `crates/controller`: the album-db worker thread — ✅ done

Landed `crates/controller/src/album.rs` (the worker thread, matching
`spec.md` §7.2) and the `config.rs` restructure (§7.5: `Config::db_path` is
now a plain `PathBuf`, via a private `ResolvedConfig` that still carries the
`Option` until `validate_db_path` confirms it). `controller` now depends on
`albumdb`.

**Two implementation deviations, folded into `spec.md` (§7.1, §7.2) — nothing
further to do about them:**
- `SetCurrentAlbum`/`SetRating`/`SetFlag` all read and emit **immediately and
  unconditionally** on receipt, not gated by the poll's "only if changed"
  dedup — per the architecture diagram in spec §6.1, which a one-line gloss
  elsewhere in the spec read ambiguously against.
- `apply_pragmas`'s upgraded journal-mode check accepts `"memory"` as well as
  `"delete"` (see step 1's note — same underlying fact).

**One thing worth knowing for step 3/4:** `mod album;` is already in
`main.rs` (needed for `album.rs` to compile at all) and `menu::year_of` (spec
§4.2) already exists **with its full test coverage** — pulled forward from
step 3 because `AlbumInfo::year` needs it. Step 3's checklist item for
`year_of` is already satisfied; step 4's `mod album;` bullet is too, it just
still needs the rest of that step's wiring.

**Verification:** `cargo build -p controller` succeeds (album.rs unused,
`#[allow(dead_code)]` still in place — remove in step 4); full workspace
quality gate green — 70 controller tests (+4 `year_of`), 51 albumdb, 44
album-tool, 9 mpd-conf; clippy clean.

---

## Step 3 — `crates/controller/src/menu.rs`: pure model additions — ✅ done

Everything unit-testable: the summary-row text, the submenu's checked state,
and the two new menu-id families. `render()` grows last, once the model
underneath it is settled — per the module's existing rule, no decisions go in
`render()`, so this step's tests are what actually prove correctness.

- [x] `year_of(original_date: Option<&str>, date: Option<&str>) -> Option<&str>`
  (spec §4.2) — **already landed in step 2** (`AlbumInfo::year` needed it
  early), tests included. Nothing to do here.
- [x] `stars(rating: u8) -> String` — five glyphs, `★`/`☆` (U+2605/U+2606).
- [x] `AlbumControls { rating: Option<u8>, relisten: bool, classical: bool, spoken: bool, soundtrack: bool }`
  and `AlbumSection { summary: Option<String>, notice: Option<String>, controls: Option<AlbumControls> }`.
- [x] `album_summary(info: &AlbumInfo) -> Option<String>` (spec §4.1): joins
  present parts with `" · "` in fixed order (year, stars, `↻` relisten,
  flag words); `None` if every part is absent.
- [x] `album_section(state: &AlbumState, write_error: Option<&str>) -> AlbumSection`
  (spec §8's table): maps each `AlbumState` variant plus the write-error flag
  to the right `summary`/`notice`/`controls` combination. `write_error`
  overrides the state-derived notice (see deviation note below); `summary`
  and `controls` are unaffected by it, so a failed write still shows the row's
  real, unchanged state alongside the notice.
- [x] `MenuModel` gains `pub album_section: AlbumSection`; `MenuModel::empty()`
  sets it to the empty/absent case. `show_track_section()`'s `||` chain gains
  `!self.album_section.is_empty()` (`AlbumSection::is_empty()` checks all
  three fields, so a `notice`-only row still shows the section).
- [x] New id prefixes `rating:` (`rating:4`) and `flag:` (`flag:classical:1`),
  alongside the existing `album:`/`addto:`/`copy:` — builders and
  `strip_rating_id`/`strip_flag_id` parsers live here, next to the existing
  ones, with a comment noting the collision hazard from spec §7.3.
- [x] `render()`: added the disabled summary/notice row(s) and the `Album ▸`
  submenu (`Rating ▸` with six `CheckMenuItem`s 0–5 prefixed by `stars()`, a
  separator, then the four flag `CheckMenuItem`s) per spec §3's worked
  example.

**Implementation deviations, folded into `spec.md` §7.3:**
- **Field-name collision, resolved by renaming both sides.** `MenuModel`
  already had a field called `album` (the album-copy row, `Option<CopyRow>`).
  Rather than let the new `AlbumSection` field shadow it, the existing field
  is now `album_title` and the new one is `album_section` — not the bare
  `album` the spec prose used. Confirmed with Korny before implementing.
- **`rating_checked(controls: &AlbumControls, rating: u8) -> bool`** was
  pulled out as its own pure function (not listed in spec §7.3's function
  list) so the "is this rating item the checked one" decision lives outside
  `render()`, per the module's own rule, and so spec §10's "`AlbumControls`
  with `rating: None`/`Some(n)`" test target has something concrete to call.
- **`write_error` priority in `album_section`**: not fully specified by
  spec §8's table (which lists the three notice conditions as if mutually
  exclusive). Implemented as: `write_error` present → `"album database write
  failed"`, overriding even the `Unavailable`-state notice; otherwise the
  state's own notice. `summary`/`controls` always follow `state` regardless of
  `write_error` — this is what makes "the re-read leaves the tick unchanged"
  (spec §8) visible.
- Two new unused-until-step-4 items — `album_section()` and
  `strip_rating_id`/`strip_flag_id` — carry a targeted `#[allow(dead_code)]`
  with a comment pointing at step 4, rather than a module-wide allow (menu.rs,
  unlike album.rs, is mostly already live).

**Unit tests** (spec §10's list, in full):
- [x] `year_of` — already landed in step 2, see above.
- [x] `stars` — 0 through 5.
- [x] `album_summary` — the full worked-example table in spec §4.1, including
  the omit-the-row case and `rating = 0` rendering as `☆☆☆☆☆` (not omitted).
- [x] `album_section` — each `AlbumState` variant → correct combination;
  `Found` yields `controls`, the other three don't; plus a `write_error`
  interaction test (see deviation note above).
- [x] `AlbumControls` with `rating: None` leaves every rating item unchecked;
  `rating: Some(n)` checks exactly the matching one — via `rating_checked`.
- [x] `strip_rating_id`/`strip_flag_id` — valid ids, malformed payloads, ids
  belonging to the other prefixes (extended the existing
  `id_parsers_reject_the_other_family_and_static_ids` test).
- [x] `MenuModel::show_track_section()` with only album data present (no
  track, no playlists) — still `true`; plus a notice-only variant.

**Verification:** `cargo test -p controller` green — 81 tests (was 74 after
step 2, +7 new). Full workspace quality gate green (44 album-tool, 51 albumdb,
81 controller, 9 mpd-conf; fmt clean; clippy clean). `render()` itself stays
unverified by `cargo test` (structurally impossible, per
`docs/architecture.md`) — covered instead by step 4's manual checklist once
`main.rs` actually calls it with live data.

---

## Step 4 — `crates/controller/src/main.rs`: wiring, then manual QA

Connects the worker (step 2) and the model (step 3) to the event loop. This is
the step where the feature becomes observable, so its verification is the
manual checklist from spec §10 in full — there's no meaningful unit-testable
surface left after step 3.

- [x] `mod album;` — already landed in step 2 (`album.rs` needed it to compile
  at all). Nothing to do here.
- [ ] New `UserEvent::Album(AlbumEvent)` variant.
- [ ] Spawn the worker as a plain `std::thread` (not tokio — blocking,
  long-lived, owns a `!Sync` resource) right after the existing
  `runtime.spawn(mpd::run(...))`, with its own `EventLoopProxy` clone and an
  `mpsc::channel()` whose `Sender` is held alongside `current_client` etc.
- [ ] Hold `current_album_path: Option<String>` — the path the menu's
  `Album ▸` submenu was built from, so a `rating:`/`flag:` click knows what to
  write. Same pattern as the existing `current_url`.
- [ ] Hold the last `MenuModel` across event-loop iterations (the one real
  refactor per spec §7.4): on an `MpdEvent`, rebuild everything as today, then
  carry the *previous* model's `album` field onto the new one; on
  `UserEvent::Album`, replace only `model.album` (via `menu::album_section`)
  and re-render. `MenuModel` is already fully owned data, so this needs no
  lifetime work — a plain `MenuModel` local, reassigned each branch.
- [ ] On every `MpdEvent`, send
  `AlbumRequest::SetCurrentAlbum(album_key(url).map(str::to_string))` (or
  `None` when nothing's playing/library-root) to the worker.
- [ ] Dispatch `rating:`/`flag:` menu-event ids (via `menu::strip_rating_id`/
  `strip_flag_id`) to `AlbumRequest::SetRating`/`SetFlag`, using
  `current_album_path`; log-and-ignore (matching the existing `addto:`
  no-current-song handling) if there's no current album to act on.
- [ ] Remove the temporary `#[allow(dead_code)]` from `album.rs` now that
  `main.rs` uses every public item in it.

**Manual verification** (spec §10, run against the real `albums.db` — back it
up first, since this is the first step that writes to it):
1. [ ] Play an album with a rating and a year → summary row appears correctly.
2. [ ] Play an album with no rating/year/flags → no summary row.
3. [ ] Set a rating from the menu; reopen; tick moved. Confirm with
   `sqlite3 <music_dir>/albums.db 'select rating, rated_at, needs_relisten from albums where path = ...'`.
4. [ ] Tick `Relisten`, then set a rating → relisten tick is gone (spec §5.1's
   accepted tradeoff).
5. [ ] Toggle `Classical`, `Spoken`, `Soundtrack` individually; confirm in SQL.
6. [ ] Run `album-tool scan` while the app is running → menu reflects any
   change within ~3s.
7. [ ] Edit a rating with `sqlite3` directly → menu follows within ~3s.
8. [ ] `chmod 000 albums.db` for a few seconds → `album database unavailable`
   row appears, then clears once permissions restore; playback controls kept
   working throughout.
9. [ ] Switch the database to WAL from another tool
   (`sqlite3 albums.db 'PRAGMA journal_mode=WAL;'`), edit a rating there,
   confirm the menu still follows within ~3s. Then restart the app with
   nothing else holding the file, confirm `albums.db-wal` is gone and the mode
   is back to `delete`.
10. [ ] Play a track at the library root (no parent directory) → no album
    rows, no panic.

**Verification**
- [ ] Full workspace quality gate green.
- [ ] All ten manual checks above pass against the real database.

---

## Step 5 — documentation

Pure doc updates, no code changes. Left last so it describes the
as-implemented behaviour rather than the as-planned one.

- [ ] `README.md`: add album display/editing to the feature list; note the
  accepted limitation from spec §6.3 (restart after restoring `albums.db`
  from a backup).
- [ ] `docs/album-database.md`: update "Consumed by hand in SQL, by agent
  skills, and eventually by the menu-bar app" — add a section on the app's
  read/write path, `open_existing`, and the `set_rating` → `needs_relisten`
  interaction.
- [ ] `docs/architecture.md`: add the album-db thread to the module table and
  the threading-model section/diagram; correct the "controller doesn't depend
  on albumdb" line.
- [ ] `docs/usage.md`: extend the menu walkthrough; state the rating/relisten
  interaction explicitly (spec §5.1) so it reads as intended behaviour.
- [ ] `docs/future_expansions.md`: remove the "doesn't read the database yet"
  note; add the deferred `mpdcontrol://` album verbs (spec §12), including the
  `command::dispatch` ordering hazard noted there.
- [ ] Confirm `crates/controller/src/config.rs`'s stale field comment was
  already deleted in step 2 (cross-check, not new work).

**Verification**
- [ ] Read each changed doc file back top-to-bottom for internal consistency
  (no lingering "not yet implemented" language).
- [ ] Full workspace quality gate green (docs-only, but cheap to confirm
  nothing else drifted).

---

## Explicitly not re-litigated here

Everything in spec.md §12 (Deferred) stays deferred — `mpdcontrol://` rating
verbs, other submenu columns, detecting a replaced `albums.db` file. Don't
add them as "small extra steps" without a separate decision to do so.
