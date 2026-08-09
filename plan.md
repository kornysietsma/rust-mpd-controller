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
- `controller` does not currently depend on `albumdb` (`docs/architecture.md`
  says so explicitly) — step 2 adds that `Cargo.toml` dependency.
- No GUI test harness exists or is being added (`muda::Menu::new` panics off
  the main thread). Pure logic is unit-tested as it's written; the worker
  thread, `render()`, and the `data_version` poll are verified by running the
  app, per the manual checklist in step 4.
- Threads talk over `std::sync::mpsc` (already a transitive dependency via
  `std`, nothing new to add to `Cargo.toml`) — a channel is explicit about the
  serialisation that an `Arc<Mutex<Connection>>` would leave implicit (spec
  §6.1).

---

## Step 1 — `crates/albumdb`: write API for the menu's edits

Everything storage-side the controller will call. No controller code yet —
this step is self-contained and testable against an in-memory database.

- [x] `connection.rs`: add `open_existing(path: &Path) -> Result<Connection>` —
  applies pragmas via the existing `apply_pragmas`, runs no schema. Uses
  `Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)` (no
  `SQLITE_OPEN_CREATE`), so a missing file errors instead of being silently
  created.
- [x] `connection.rs`: upgrade `apply_pragmas`'s `journal_mode` set to
  `pragma_update_and_check`, asserting the returned mode is `"delete"`
  (case-insensitive compare, matching the existing pragma tests' style).
  **Deviation:** also accepts `"memory"` — `:memory:` databases always report
  `journal_mode = memory` regardless of what's requested (SQLite can't journal
  in-memory databases any other way), so a strict `"delete"`-only assertion
  broke `open_in_memory()` and every test built on it. Confirmed via the
  existing `pragmas_read_back_correctly` test, which already asserted this
  behaviour before this change.
- [x] `enums.rs`: add `AlbumFlag { Relisten, Classical, Spoken, Soundtrack }`
  with `column(self) -> &'static str` mapping to `needs_relisten`,
  `is_classical`, `is_spoken`, `is_soundtrack`. Not a `string_enum!` — it
  names columns, not a schema string value.
- [x] `queries.rs`: add `set_flag(conn, album_id, flag: AlbumFlag, value: bool) -> Result<()>`.
  Column name is formatted into the SQL string — comment *why* that's safe
  (closed enum, never caller data), so a reviewer doesn't flag it as
  injection.
- [x] `queries.rs`: remove `set_needs_relisten`; it's fully superseded by
  `set_flag(.., AlbumFlag::Relisten, ..)`.
- [x] `queries.rs`: `set_rating(Some(value))`'s `UPDATE` also sets
  `needs_relisten = 0`, in the same statement as `rating`/`rated_at`. Update
  the doc comment. `set_rating(None)` is unchanged (untouched `needs_relisten`,
  per spec §5.1 — unreachable from the menu but still part of the API).
- [x] `lib.rs`: export `open_existing`, `AlbumFlag`, `set_flag`; drop
  `set_needs_relisten` from the export list.
- [x] `crates/album-tool/src/status.rs`: update its one call site — the test
  `report_covers_tag_states_sync_states_missing_ratings_and_relisten` — to use
  `set_flag(&conn, relisten, AlbumFlag::Relisten, true)` instead of
  `set_needs_relisten`.

**Unit tests (in-memory DB, `albumdb`'s existing style):**
- [x] `set_flag` writes each of the four columns, both directions.
- [x] `set_rating(Some(n))` sets `rating`/`rated_at` **and** clears
  `needs_relisten` — landed as a new standalone test
  (`set_rating_some_clears_needs_relisten`) next to the existing
  `set_rating_clears_rating_but_keeps_rated_at` rather than extending that
  test in place, to keep each test asserting one behaviour.
- [x] `set_rating(None)` still leaves `needs_relisten` alone — same treatment,
  as `set_rating_none_leaves_needs_relisten_alone`.
- [x] `open_existing` succeeds against a database created by `open`.
- [x] `open_existing` on a non-existent path errors, and no file is created —
  assert with `path.exists()` after the call.
- [x] `apply_pragmas`' upgraded journal-mode check still passes the existing
  `journal_mode_on_disk_is_delete` test unmodified (regression guard on the
  `pragma_update_and_check` change).

**Verification**
- [x] `cargo test -p albumdb` and `cargo test -p album-tool` both green.
- [x] Full workspace quality gate (fmt/clippy/test) green.

---

## Step 2 — `crates/controller`: the album-db worker thread

Adds the dependency on `albumdb`, restructures `Config::db_path`, and lands
the whole worker module. No `menu.rs`/`main.rs` changes yet — the worker is
buildable and its types are usable in isolation (`main.rs` will just not spawn
it until step 4).

- [ ] `Cargo.toml`: add `albumdb = { path = "../albumdb" }` to
  `crates/controller`'s dependencies.
- [ ] `config.rs`: restructure so `load()` returns `Config` with
  `db_path: PathBuf` (not `Option<PathBuf>`) — `resolve()` keeps the `Option`
  internally, and `validate_db_path` (or a renamed equivalent) is what turns
  `None` into `ConfigError::DbLocationUnknown` before `load` ever returns.
  Delete the stale `db_path` field comment ("Not yet read by anything...").
  Update the existing `config.rs` tests for the new return type — most just
  drop an `Option`-unwrap layer.
- [ ] New `crates/controller/src/album.rs`:
  - `AlbumRequest { SetCurrentAlbum(Option<String>), SetRating { path, rating: u8 }, SetFlag { path, flag: AlbumFlag, value: bool } }`
  - `AlbumInfo { year: Option<String>, rating: Option<u8>, relisten: bool, classical: bool, spoken: bool, soundtrack: bool }`
  - `AlbumState { NoAlbum, Unavailable, NotFound, Found(AlbumInfo) }`
  - `AlbumEvent { state: AlbumState, write_error: Option<String> }`
  - `pub fn run(db_path: PathBuf, rx: Receiver<AlbumRequest>, on_event: impl Fn(AlbumEvent))` —
    mirrors `mpd::run`'s shape. Owns the `rusqlite::Connection` from inside the
    thread (never crosses it — `Connection` is `Send`, not `Sync`).
  - Worker loop per spec §6.2: `recv_timeout` against a `next_poll` deadline
    (not a bare `POLL_INTERVAL`), so a burst of requests can't starve the
    poll. `POLL_INTERVAL = Duration::from_secs(3)`.
  - `handle(request)`: `SetCurrentAlbum` just remembers the path for the poll
    to watch; `SetRating`/`SetFlag` write then re-read the row and emit.
  - `poll()`: reads `PRAGMA data_version`, compares to the last-seen value;
    unchanged → nothing, changed → re-read the watched row and emit only if
    the resulting `AlbumState` differs from the last one sent. Pragma error →
    drop the connection, emit `Unavailable`, retry (re-open, re-baseline
    `data_version`) next tick.
  - Uses `open_existing` — never `open` — so this thread can never create or
    migrate `albums.db`.
- [ ] Module-header comment on `album.rs` stating (per repo convention, see
  `menu.rs`'s own note) that the worker loop and the `data_version` poll are
  **not** unit-tested — verified by running the app (step 4's manual
  checklist) — so a future reader doesn't go looking for missing coverage.

**Verification**
- [ ] `cargo build -p controller` succeeds with `album.rs` compiled in but
  unused (an explicit `#[allow(dead_code)]` or similar is fine short-term;
  remove it in step 4 once `main.rs` wires the module in — don't let it linger
  past that step).
- [ ] `config.rs`'s existing test suite passes against the new return type.
- [ ] Full workspace quality gate green.

---

## Step 3 — `crates/controller/src/menu.rs`: pure model additions

Everything unit-testable: the summary-row text, the submenu's checked state,
and the two new menu-id families. `render()` grows last, once the model
underneath it is settled — per the module's existing rule, no decisions go in
`render()`, so this step's tests are what actually prove correctness.

- [ ] `year_of(original_date: Option<&str>, date: Option<&str>) -> Option<&str>`
  (spec §4.2): first four bytes of `original_date`, falling back to `date`,
  guarded to require all four ASCII digits.
- [ ] `stars(rating: u8) -> String` — five glyphs, `★`/`☆` (U+2605/U+2606).
- [ ] `AlbumControls { rating: Option<u8>, relisten: bool, classical: bool, spoken: bool, soundtrack: bool }`
  and `AlbumSection { summary: Option<String>, notice: Option<String>, controls: Option<AlbumControls> }`.
- [ ] `album_summary(info: &AlbumInfo) -> Option<String>` (spec §4.1): joins
  present parts with `" · "` in fixed order (year, stars, `↻` relisten,
  flag words); `None` if every part is absent.
- [ ] `album_section(state: &AlbumState, write_error: Option<&str>) -> AlbumSection`
  (spec §8's table): maps each `AlbumState` variant plus the write-error flag
  to the right `summary`/`notice`/`controls` combination.
- [ ] `MenuModel` gains `pub album: AlbumSection`; `MenuModel::empty()` sets it
  to the empty/absent case. `show_track_section()`'s `||` chain gains
  `self.album.summary.is_some() || self.album.controls.is_some()` (a
  `notice`-only row still needs the section to show, so include that arm too —
  confirm against spec §8's "database unavailable" row, which must render even
  with nothing else in the section).
- [ ] New id prefixes `rating:` (`rating:4`) and `flag:` (`flag:classical:1`),
  alongside the existing `album:`/`addto:`/`copy:` — builders and
  `strip_rating_id`/`strip_flag_id` parsers live here, next to the existing
  ones. Note in a comment that the new `flag:` prefix must never start with
  `album` (the existing `strip_album_id` uses `strip_prefix` unambiguously
  today, but a careless future rename could collide).
- [ ] `render()`: add the disabled summary/notice row(s) and the `Album ▸`
  submenu (`Rating ▸` with six `CheckMenuItem`s 0–5 prefixed by `stars()`, a
  separator, then the four flag `CheckMenuItem`s) per spec §3's worked
  example. No new decisions here — it walks `AlbumSection` exactly as the rest
  of `render()` walks `MenuModel`.

**Unit tests** (spec §10's list, in full):
- [ ] `year_of` — `YYYY`, `YYYY-MM`, `YYYY-MM-DD`, `original_date` absent
  (falls back to `date`), both absent, non-numeric leading four chars.
- [ ] `stars` — 0 through 5.
- [ ] `album_summary` — the full worked-example table in spec §4.1, including
  the omit-the-row case and `rating = 0` rendering as `☆☆☆☆☆` (not omitted).
- [ ] `album_section` — each `AlbumState` variant → correct combination;
  `Found` yields `controls`, the other three don't.
- [ ] `AlbumControls` with `rating: None` leaves every rating item unchecked;
  `rating: Some(n)` checks exactly the matching one.
- [ ] `strip_rating_id`/`strip_flag_id` — valid ids, malformed payloads, ids
  belonging to the other prefixes (extend the existing
  `id_parsers_reject_the_other_family_and_static_ids` test).
- [ ] `MenuModel::show_track_section()` with only album data present (no
  track, no playlists) — still `true`.

**Verification**
- [ ] `cargo test -p controller` green, new tests included.
- [ ] Full workspace quality gate green.
- [ ] `render()` itself stays unverified by `cargo test` (structurally
  impossible, per `docs/architecture.md`) — covered instead by step 4's
  manual checklist once `main.rs` actually calls it with live data.

---

## Step 4 — `crates/controller/src/main.rs`: wiring, then manual QA

Connects the worker (step 2) and the model (step 3) to the event loop. This is
the step where the feature becomes observable, so its verification is the
manual checklist from spec §10 in full — there's no meaningful unit-testable
surface left after step 3.

- [ ] `mod album;` alongside the existing `mod` declarations.
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
