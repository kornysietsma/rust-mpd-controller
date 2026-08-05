# Album Database — Implementation Plan

**Source spec:** [`spec.md`](spec.md)
**Date:** 2026-08-04

Work through the steps in order. Each is independently completable, leaves the
tree building and green, and ends at a natural commit point.

---

## 1. Technical context

### 1.1 Decisions made when planning

| Decision | Choice | Why |
|---|---|---|
| Test fixtures | Small **synthetic** fixture committed to git | `/album-data` is gitignored and the repo is shared. Invented album names keep the library private, and each awkward case is present deliberately rather than by luck. |
| Real-library verification | `album-tool status` + **manual checklist** per step | Keeps `cargo test` hermetic and fast. Matches how this project already verifies its GUI paths. |
| Schema management | Plain `CREATE TABLE IF NOT EXISTS`, changes applied by hand | Single-user tool. `ALTER TABLE` preserves data, so no framework earns its keep. See §1.4. |
| Ratings import | **Throwaway Python script, no tests** | Runs exactly once against real data. Building it in Rust with dry-run, `--force` and sanity-check machinery would be ceremony for a job that happens once. |
| Clock | **Injected**, never called inline | Standards forbid embedding `now()`. Needed for `first_seen`/`last_seen`/`rated_at`/`mb_synced_at` and to test rate limiting without sleeping. |
| Commits | Author makes their own | Each step ends with a verification pass, then hand over. |

### 1.2 The testability boundary

The single most important structural decision, and it follows a lesson already
learned in this codebase (`AGENTS.md`): **`mpd_client`'s `Song`/`Status` are
`#[non_exhaustive]` with no public constructor, so they cannot be built in a
test.**

Therefore:

```
  MPD  ──parse──▶  Vec<SongTags>  ──pure──▶  AlbumFacts  ──▶  albumdb
       (I/O, thin)   plain struct    (all the logic)          (sqlite)
```

- `SongTags` is a plain owned struct (`path: String`, `tags: BTreeMap<String, Vec<String>>`
  or named fields) defined in `albumdb`. It is *not* an `mpd_client` type.
- Everything interesting — grouping into albums, `tag_state` derivation,
  title/artist resolution, fingerprinting — is a **pure function over
  `Vec<SongTags>`** and is fully testable with hand-built values.
- The MPD layer's only job is producing `Vec<SongTags>`. It sits behind a
  trait so the fixture can substitute for it.

The same shape applies to MusicBrainz: a `MusicBrainzClient` trait, a real HTTP
implementation, and a fake returning canned responses.

### 1.3 Traits to inject

```rust
trait LibrarySource   { fn songs(&self) -> Result<Vec<SongTags>>; }
trait MusicBrainzClient { async fn release_group(&self, mbid: &str) -> Result<ReleaseGroup>; }
trait Clock           { fn now(&self) -> OffsetDateTime; }
```

Three implementations each: real, fixture/fake, and (for `Clock`) a fixed test
clock. Nothing else needs mocking.

### 1.4 Schema changes

No migration mechanism, no version marker. This is a single-user tool, so a
schema change is a manual operation: back up the file (or `.dump` it to SQL),
then `ALTER TABLE ADD COLUMN`, or the standard create/copy/drop/rename
procedure for the rarer cases SQLite can't alter in place, such as changing a
`CHECK` constraint.

Both routes preserve existing data. **A schema change does not mean rebuilding
the database**, so hand-entered values are never at risk from one.

The deferred `verify` columns (`mb_verify_*`) are in the schema from day one
anyway, so adding `verify` later needs no change at all.

### 1.5 Error handling

`anyhow::Result` at the CLI boundary, matching the existing crate. Expected,
unsurprising conditions are **values, not errors**: a MusicBrainz 404 sets
`mb_sync_state = 'failed'` with a reason, it does not abort the run. An
unreadable database is a genuine error and aborts.

### 1.6 Quality gates

There is no CI in this repo, so these are run by hand before each step is
considered done:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Default rustfmt and clippy settings — the project has no config files for
either, and this plan doesn't add any.

---

## 2. Steps

### Step 1 — Spike: confirm two crate APIs

**Throwaway work.** Both unknowns block design decisions later, so resolve them
before building anything on top.

Per `AGENTS.md`, read the vendored source rather than guessing:
`find ~/.cargo/registry/src -maxdepth 2 -iname "<crate>-*"`

- [ ] Does `musicbrainz_rs` 0.13.0 fetch a **release group** with
      `inc=genres` and expose genre **vote counts**?
- [ ] Can `mpd_client` 1.4.1 read arbitrary tags — specifically
      `MUSICBRAINZ_RELEASEGROUPID` and `OriginalDate` — or does it only
      surface a fixed typed set?

**Fallbacks, both already established patterns:**
- MusicBrainz → direct `reqwest` against
  `/ws/2/release-group/<mbid>?inc=genres&fmt=json`, which spec §2.2 already
  validated end to end.
- MPD → hand-rolled `RawCommand`, exactly as `mpd.rs` already does for
  `listplaylist` and `command::AddRecursive`.

**Verify:** fetch release group `e159ede0-…` (Alice Cooper, *Welcome to My
Nightmare*) and confirm you get `hard rock(9)`, `dark cabaret(5)`, `rock(5)` —
the values measured in spec §2.2. Anything else means the wrong endpoint or
`inc=` parameter.

**Done when:** both answers are recorded in this file under §1.1, and the
spike code is deleted.

---

### Step 2 — Workspace split

The highest-regression-risk step in the plan, and it changes no behaviour.

- [ ] Create workspace root `Cargo.toml`.
- [ ] Move `src/*.rs` → `crates/controller/src/`, verbatim. **No code changes.**
- [ ] Move `[package.metadata.bundle]`, `osx_url_schemes` and
      `osx_info_plist_exts` to `crates/controller/Cargo.toml`. Check
      `assets/` paths still resolve.
- [ ] Create empty `crates/albumdb` and `crates/album-tool`.
- [ ] Check `install.sh` for hard-coded paths into `src/` or `target/`.

**Automated:** `cargo test --workspace` — all **56** existing tests still pass
(38 in `menu`, 9 in `command`, 5 in `main`, 4 in `config`).

**Manual — this is the part that matters.** A `cargo run` success proves
nothing here; several known bugs in this app only reproduce in the installed
bundle (see `AGENTS.md`, "load-bearing oddities"). So:

- [ ] Build and install the bundle via `install.sh`.
- [ ] Launch it. Confirm the tray icon appears and the app stays out of the
      Dock (`ActivationPolicy::Accessory` + `LSUIElement`).
- [ ] Open the menu: playback controls, album traversal, playlist membership,
      "Add to", and both copy-URI rows are present and correct.
- [ ] Trigger an `mpdcontrol://` URL and confirm it acts on MPD.

Per `AGENTS.md`, **ask what the tray shows** rather than screenshotting.

**Done when:** the app behaves identically to before the move. If anything
regressed, fix it here — do not carry it forward.

---

### Step 3 — `albumdb`: schema and connection

- [ ] `rusqlite` 0.40.1 with `bundled`.
- [ ] `sql/schema.sql` — the tables from spec §4.2 with `IF NOT EXISTS`.
- [ ] `open(path)` applying `journal_mode=DELETE`, `foreign_keys=ON`,
      `busy_timeout=5000`.
- [ ] `open_in_memory()` for tests, same schema.

**Automated:**
- [ ] Opening a fresh DB creates both tables with every column from spec §4.2.
- [ ] Opening twice is idempotent and preserves data.
- [ ] All three pragmas read back correctly — **especially
      `journal_mode = delete`**, since the whole point is that no `-wal`
      sidecar is ever created for Dropbox to capture inconsistently.
- [ ] Every `CHECK` constraint rejects a bad value: `rating = 6`,
      `tag_state = 'nonsense'`, `match_policy = 'maybe'`.
- [ ] `ON DELETE CASCADE` removes an album's genre rows.

**Manual:** create a DB on disk; confirm no `.db-wal` or `.db-shm` appears
alongside it.

---

### Step 4 — `albumdb`: domain types and pure logic

All pure, all unit-testable, no I/O. These are the "small complex functions
with real complexity" that the standards say unit tests are *for*.

- [ ] `SongTags` plain struct (§1.2).
- [ ] Enums: `TagState`, `TitleSource`, `MatchPolicy`, sync/verify states, with
      `as_str`/`from_str` matching the `CHECK` constraints exactly.
- [ ] `derive_tag_state(&[SongTags]) -> TagState` per spec §5.4.
- [ ] `normalise_date` — `'0000'` and empty → `None`.
- [ ] `parse_album_path(&str) -> (title, artist)` per spec §5.3.
- [ ] `TrackRange` — parse and format `'1-10'`, `'1-12,14'`, `None`.
- [ ] `fingerprint(&[SongTags]) -> String` per spec §5.2.

**Automated:**
- [ ] `derive_tag_state` — one test per state, all six. Include the trap
      explicitly: **many artists with a single `AlbumArtist` and one year is
      `no_mbid_consistent`, not `varied`** (spec §2.5).
- [ ] `Date = '0000'` alone → `None`; mixed `['0000', '1787']` → treated as
      one year, not a variation.
- [ ] Path parsing for all four patterns in spec §5.3, plus a path containing
      an apostrophe.
- [ ] `TrackRange` round-trips: `parse(format(x)) == x`. Rejects `'0-5'`,
      `'5-1'`, `'abc'`, `''`.
- [ ] `fingerprint` is stable under track reordering, and differs when the
      track set differs.

---

### Step 5 — `albumdb`: queries

- [ ] `upsert_album` — writes derived fields, **never touches user-editable
      columns**.
- [ ] `find_by_path`, `find_by_fingerprint`, `all_present`.
- [ ] `rename_album(album_id, new_path)`.
- [ ] `mark_missing(album_id)`.
- [ ] `replace_genres(album_id, source, genres)` — replaces one source, leaves
      the other alone.
- [ ] User-field setters: rating, flags, `original_tracks`, `match_policy`.

**Automated** (acceptance-style against in-memory SQLite, per the standards'
preference for module-level tests):
- [ ] **The load-bearing invariant:** set every user-editable column, run
      `upsert_album` with completely different derived data, assert all of
      `rating`, `needs_relisten`, `is_*`, `original_tracks`, `match_policy`,
      `notes` are unchanged. This is the one test that must never be deleted.
- [ ] `replace_genres(.., 'mb', ..)` leaves `source = 'tag'` rows intact, and
      vice versa.
- [ ] Re-running `replace_genres` with the same source doesn't duplicate rows.
- [ ] `rename_album` preserves `album_id` and all user fields.

---

### Step 6 — `album-tool` skeleton

- [ ] `clap` 4.6.5 derive: `scan`, `sync`, `status` as stubs.
- [ ] TOML config: database path and MPD address.
- [ ] **The database path has no default and is required** — MPD's
      `music_directory` is not discoverable over TCP (`config` is rejected for
      non-local clients, spec §2.7), so it cannot be inferred.

**Automated:** config parsing, including a clear error when the path is absent.

**Manual:** `album-tool --help` and each `--help` read sensibly; running with
no config gives an actionable message, not a panic.

---

### Step 7 — MPD library reader

- [ ] `LibrarySource` trait (§1.3).
- [ ] Real implementation: `listfiles` → top-level dirs, `lsinfo` per top →
      chunk boundaries, `find "(base '<chunk>')"` per chunk, de-duplicated by
      path.
- [ ] **Two-level quoting** helper (spec §2.7) — escape for the inner
      single-quoted filter, then again for the outer double-quoted argument.
- [ ] Fixture implementation reading the committed fixture file.
- [ ] Write `crates/album-tool/tests/fixtures/library.txt`: ~20 invented
      albums covering all six `tag_state` values, `Date '0000'`, a multi-disc
      album, a missing `AlbumArtist`, and a path with an apostrophe.

**Automated:**
- [ ] Quoting helper unit tests — this is regex-grade fiddly logic and exactly
      what unit tests are for. Must handle
      `A Wake-Up Call for the '90s_ Epic_Associated Hard Rock Sampler`, which
      is a real path in the library and broke the first attempt.
- [ ] Response parsing produces the expected `SongTags`.
- [ ] De-duplication when a top-level chunk overlaps its own subdirectories.

**Manual against live MPD:**
- [ ] Total songs read = **29,028**; distinct directories = **1,886**.
- [ ] Do **not** try `find "(base '')"` over the whole library — it returns
      zero rows because the response is too large. Chunking is not an
      optimisation, it is required.

---

### Step 8 — `scan`: grouping, `tag_state`, title/artist

No reconciliation yet. Every scan is treated as a first scan.

- [ ] Group songs by containing directory.
- [ ] Derive `tag_state`, resolve title/artist and `title_source` (spec §5.3).
- [ ] Populate dates, label, `track_count`, `disc_count`, MB identifiers.
- [ ] Write the file-tag genre as `source = 'tag'`, `votes = 0`.
- [ ] `--dry-run` reports without writing.

**Automated:** full scan of the fixture into an in-memory DB, asserting the
`tag_state` for each fixture album and that `title_source` is `path` for the
varied/incomplete ones and `tag` otherwise.

**Manual against live MPD** — these are the acceptance numbers from spec §5.4:

```
$ album-tool scan --dry-run
$ album-tool status

expect: 1886 albums
        clean               1833
        mixed_mbid             2
        partial_mbid           1
        no_mbid_consistent    42
        no_mbid_varied         5
        no_mbid_incomplete     3
```

- [ ] Counts match exactly. A discrepancy means the derivation is wrong, or
      the library changed since 2026-08-04 — investigate which before moving on.
- [ ] Spot-check the named cases from spec §2.5: `Songs for Becky` and
      `Christmas Music Compilation` are `no_mbid_varied`; `Unnmatched/Madness`
      is `no_mbid_incomplete`.
- [ ] A second `scan` reports zero changes.

---

### Step 9 — `scan`: reconciliation

- [ ] Diff MPD paths against stored paths.
- [ ] Match disappearance ↔ appearance by fingerprint → `rename_album`.
- [ ] Unmatched disappearances → `status = 'missing'`. **Never delete a row** —
      deleting silently discards a rating.

**Automated:**
- [ ] Scan fixture, rate an album, rename its directory in a modified fixture,
      re-scan → same `album_id`, rating intact, one rename reported.
- [ ] Remove an album entirely → `status = 'missing'`, row still present,
      rating intact.
- [ ] A removal and an unrelated addition in the same scan are **not** paired
      as a rename.
- [ ] An album returning after being `missing` goes back to `present` and
      keeps its rating.

**Manual:** rename a real album directory, `mpc update`, re-scan, confirm the
rating survived. Then rename it back.

---

### Step 10 — Ratings import (Python one-off, not Rust)

**No Rust, no automated tests.** This runs once against real data; correctness
is established by looking at the result. Must run **after** a successful
`scan`, since it only updates rows that already exist.

A single `uv` script with PEP 723 inline metadata, at
**`scripts/import_ratings.py`** — deliberately *not* in `album-data/`, which is
gitignored. The script is committed for posterity; the CSV it reads is not, so
take the CSV path and the database path as **arguments** rather than hard-coding
them. No personal data enters git.

- [ ] Read `all_albums_export.csv`, take **only** the `rating` column.
- [ ] Map per spec §6: `0`–`5` → `rating` + `rated_at`; `?` →
      `needs_relisten = 1`; `c` → `is_classical = 1`; blank → no change.
- [ ] Match on exact `file` path against `albums.path`, trailing slash
      stripped.
- [ ] Ignore the one duplicate (`Compilations/Sound Environment`) — last row
      wins, correct by hand later if it matters.
- [ ] Print a summary of what it did.

No dry-run, no `--force`, no unmatched-path handling: spec §2.6 established
that **all 1,848 CSV paths already exist**, so there is no such case to design
around. If the script reports otherwise, stop — the library has moved and that
is worth understanding, not working around.

**Verify by inspecting the data** (spec §2.6):
- [ ] 416 ratings, 69 `is_classical`, 17 `needs_relisten` — via
      `album-tool status` or plain SQL.
- [ ] Spot-check a handful of known albums against the spreadsheet.
- [ ] Re-run `album-tool scan` afterwards and confirm every rating survives.
      This is the real proof that the §5 invariant holds against live data.

**Lifecycle:** write it → run it → verify the data → **commit it** → delete it
in a later commit once the whole plan is done (§3). Git history keeps it for
posterity, so nothing is lost by removing it from the working tree, and the
repo doesn't carry a script that can never be usefully run again.

---

### Step 11 — `status`

- [ ] Counts by `tag_state` and `mb_sync_state`.
- [ ] Albums with `status = 'missing'`.
- [ ] `mixed_mbid` / `partial_mbid` albums listed **for manual fixing in
      Picard** — the tool reports, it never edits files.
- [ ] Failed syncs with their errors.
- [ ] Rating coverage and `needs_relisten` count.

**Automated:** seeded in-memory DB produces the expected report.

**Manual:** run against the live database; confirm the two known `mixed_mbid`
albums appear — `Billy Bragg/The Million Things That Never Happened` and
`Dire Straits/Brothers in Arms`.

---

### Step 12 — `sync`: transport

Transport and discipline only; persistence is step 13.

- [ ] `MusicBrainzClient` trait plus real implementation (crate or `reqwest`,
      per step 1).
- [ ] Rate limit **≥1 s between requests**, driven by the injected `Clock` so
      tests don't sleep.
- [ ] Descriptive User-Agent with a contact address. Non-negotiable — generic
      agents get blocked.
- [ ] Retry `503`/`5xx` with exponential backoff, ≥3 attempts.
- [ ] `404` → `mb_sync_state = 'failed'` with the reason; the run continues.
- [ ] Give-up → `failed`. **Never leave a row `pending` after a real attempt**,
      or every re-run retries it forever.
- [ ] Commit state per album so an interrupted run resumes where it stopped.

**Automated, all against the fake client:**
- [ ] Rate limiting: N requests request ≥ N−1 seconds from the fake clock.
- [ ] `503` then success → succeeds after retry.
- [ ] Persistent `503` → `failed` with an error recorded, run continues.
- [ ] `404` → `failed`, run continues.
- [ ] Selection honours `match_policy = 'never'`, `status = 'missing'` and
      `mb_release_group_id IS NULL`.
- [ ] **De-duplication by release group:** two albums sharing one group cause
      exactly one request. (1,830 groups across 1,886 albums — spec §7.1.)
- [ ] Interrupting mid-run and resuming re-requests nothing already `ok`.

---

### Step 13 — `sync`: apply results

- [ ] Write `album_genres` with `source = 'mb'` and vote counts, replacing only
      `mb` rows.
- [ ] Cache `primary_genre` = highest-voted genre.
- [ ] `mb_original_count` from the earliest release in the release group
      (needs `inc=releases`). The *tagged* release includes bonus tracks and
      cannot supply this.
- [ ] Where confidently derivable and `original_tracks` is unset, set
      `'1-<n>'` with `original_tracks_src = 'mb'`. **Never overwrite
      `manual`.**
- [ ] `mb_sync_state = 'ok'`, `mb_synced_at` from the injected clock.
- [ ] Category flags are **not** written — deferred per spec §7.3.

**Automated:**
- [ ] Canned response → correct genres with votes, correct `primary_genre`.
- [ ] `source = 'tag'` genre rows survive.
- [ ] An `original_tracks_src = 'manual'` value is not overwritten.
- [ ] Re-sync with `--force` replaces `mb` genres without duplicating.

**Manual against the live API:**
- [ ] `album-tool sync --limit 5` first. Confirm ~1 s between requests and
      sensible genres before running at scale.
- [ ] Full run: ~1,830 requests, **~31 minutes**. Interrupt it partway and
      resume; confirm it doesn't restart.
- [ ] Spot-check *Welcome to My Nightmare* against spec §2.2 — `hard rock(9)`,
      `dark cabaret(5)`, `rock(5)`, and 10 genres rather than the single
      `Hard Rock` file tag.

---

## 3. Final acceptance

A full `scan` → ratings import script → `sync` against the live library, then:

- [ ] 1,886 albums with the `tag_state` distribution from spec §5.4.
- [ ] 416 ratings, 69 `is_classical`, 17 `needs_relisten`.
- [ ] ~1,830 release groups carrying MB genres, **several genres each**.
- [ ] A second `scan` changes nothing.
- [ ] **No automated command has modified any user-editable field.**
- [ ] `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test --workspace` all
      clean.
- [ ] The menu-bar app still works, installed as a bundle.
- [ ] No `.db-wal` or `.db-shm` file next to the database.
- [ ] `scripts/import_ratings.py` deleted in a final commit, having served its
      one purpose. It remains in git history.

## 4. Out of scope

Per spec §9: `verify`, `export`, all controller/menu-bar features, and
category derivation from MusicBrainz genres. The schema already carries the
columns for the first and last of these, so none needs a schema change.
