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
| MusicBrainz crate | `musicbrainz_rs` 0.13.0 with `async` feature, confirmed by spike | Fetches release-group genres with vote counts and release lists live. No `reqwest` fallback needed. Usage gotchas in `AGENTS.md`; dependency note in spec §3.3. |

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

- `SongTags` is a plain owned struct with named fields (`path: String`,
  `album: Option<String>`, ...), decided in Step 4 over a generic
  `BTreeMap<String, Vec<String>>` — the set of tags this project needs is
  fixed and known, so compiler-checked field names beat a string-keyed map.
  Defined in `crates/albumdb/src/song_tags.rs`. It is *not* an `mpd_client`
  type.
- Everything interesting — grouping into albums, `tag_state` derivation,
  title/artist resolution — is a **pure function over `Vec<SongTags>`** and
  is fully testable with hand-built values.
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

### Step 1 — Spike: confirm two crate APIs — DONE

- [x] `musicbrainz_rs` 0.13.0 fetches a release group with genre vote counts
      via `.with_genres()`, and release lists via `.with_releases()`.
      Verified live, matching spec §2.2's numbers exactly.
- [x] `mpd_client` 1.4.1 surfaces both typed tags (`OriginalDate`) and
      untyped ones (`MUSICBRAINZ_RELEASEGROUPID`, via `Tag::Other`).
      Verified live against the running server.

No `reqwest`/hand-rolled fallback needed for either. Full usage notes (feature
flags, client construction, rate limiter) are in `AGENTS.md`; the dependency
decision is recorded in spec §3.3. The spike used a scratch crate outside this
repo, so there was no in-repo throwaway code to delete.

---

### Step 2 — Workspace split — DONE

- [x] Workspace root `Cargo.toml` created (`[workspace]`, members
      `crates/controller`, `crates/albumdb`, `crates/album-tool`).
- [x] `src/*.rs` and `assets/` moved verbatim to `crates/controller/`, via
      `git mv`. No code changes.
- [x] `[package.metadata.bundle]` moved to `crates/controller/Cargo.toml`;
      bundle asset paths (`assets/icon.icns` etc.) still resolve since they're
      relative to that manifest.
- [x] Empty `crates/albumdb` (lib) and `crates/album-tool` (bin, stub `main`)
      created.
- [x] `install.sh` updated: `cargo bundle --release` → `cargo bundle --release
      -p controller` — required once the workspace root has no package of its
      own for `cargo bundle` to default to.

**Package rename:** the `controller` crate's package name is now `controller`
(was `rust-mpd-controller`), matching spec §3.2's crate table. This changes
the built binary's filename (`target/release/controller`, was
`rust-mpd-controller`) but not the bundled app name (`MpdController.app`,
set separately via `metadata.bundle.name`) — nothing else referenced the old
binary name.

**Automated:** `cargo test --workspace` — 57 passed, 0 failed (plan's original
"56" estimate was stale by one test already added in `main.rs` before this
step; not a regression from the move). `cargo fmt --all` and
`cargo clippy --workspace --all-targets -- -D warnings` both clean.

**Manual — verified against the installed bundle**, per `AGENTS.md`'s warning
that several bugs only reproduce there, not under `cargo run`: built and
installed via `install.sh`, launched, confirmed by Korny — tray icon present,
app stays out of the Dock, full menu (playback, album traversal, playlist
membership, "Add to", both copy-URI rows) intact, and an `mpdcontrol://` URL
still acts on MPD. Behaves identically to before the move.

---

### Step 3 — `albumdb`: schema and connection — DONE

- [x] `rusqlite` 0.40.1 (`bundled`) added to `crates/albumdb/Cargo.toml`.
- [x] `crates/albumdb/sql/schema.sql` — the spec §4.2 tables verbatim, with
      `IF NOT EXISTS`.
- [x] `crates/albumdb/src/connection.rs`: `open(path)` and `open_in_memory()`,
      both applying `journal_mode=DELETE`, `foreign_keys=ON`,
      `busy_timeout=5000` and the schema. Re-exported from `lib.rs` as
      `albumdb::open` / `albumdb::open_in_memory`, plus `albumdb::Connection`
      (re-export of `rusqlite::Connection`).

**Automated (8 tests, all passing):** full column presence in both tables,
idempotent re-open with data preserved, pragma readback, every `CHECK`
constraint rejects a bad value, `ON DELETE CASCADE` removes genre rows. Note
for anyone touching this later: an in-memory SQLite DB always reports
`journal_mode = memory` regardless of what's requested, so the
`journal_mode = delete` assertion runs against an on-disk DB instead.

**Manual:** confirmed on disk via a throwaway example binary (since deleted)
that only `albums.db` exists after writing and closing — no `-wal`/`-shm`
sidecar.

Toolchain gotcha hit and fixed (`rustup update`) — see `AGENTS.md`.

Quality gates clean: `cargo fmt --all`, `cargo clippy --workspace
--all-targets -- -D warnings`, `cargo test --workspace` (albumdb 8/8,
controller 57/57, album-tool 0 — still a stub).

---

### Step 4 — `albumdb`: domain types and pure logic — DONE

All pure, all unit-testable, no I/O. These are the "small complex functions
with real complexity" that the standards say unit tests are *for*.

- [x] `SongTags` plain struct (§1.2). Named fields (not a `BTreeMap`) — a
      deliberate choice discussed with Korny: the set of tags this project
      needs is fixed and known, so compiler-checked field names beat a
      string-keyed map.
- [x] Enums: `TagState`, `TitleSource`, `MatchPolicy`, sync/verify states
      (`SyncState`, `VerifyState`), with `as_str`/`FromStr` matching the
      schema's allowed values exactly — `status`, `match_policy` and
      `tag_state` via a real `CHECK` constraint, the other three via the
      schema's documentation comment (§4.2 has no `CHECK` for them).
- [x] `derive_tag_state(&[SongTags]) -> TagState` per spec §5.4.
- [x] `normalise_date` — `'0000'` and empty → `None`.
- [x] `parse_album_path(&str) -> Result<(title, artist), String>` per spec
      §5.3 — an unmatched path is an error, not a best-effort guess, per
      Korny: it means either the data is wrong or a new case exists, and
      either way it should be visible, not silently papered over.
- [x] `TrackRange` — parse and format `'1-10'`, `'1-12,14'`, `None`.

**Automated (32 new tests, all passing):**
- [x] `derive_tag_state` — one test per state, all six, including the trap:
      **many artists with a single `AlbumArtist` and one year is
      `no_mbid_consistent`, not `varied`** (spec §2.5).
- [x] `Date = '0000'` alone → `None`; mixed `['0000', '1787']` → treated as
      one year, not a variation.
- [x] Path parsing for all four patterns in spec §5.3, plus a path containing
      an apostrophe.
- [x] `TrackRange` round-trips: `parse(format(x)) == x`. Rejects `'0-5'`,
      `'5-1'`, `'abc'`, `''`.

**Revised after Korny review:** `fingerprint`/automatic rename-matching was
cut entirely, along with the `fingerprint` schema column and the `sha2`
dependency it needed. Disappearances are already soft-deleted
(`status = 'missing'`, never removed), so the only real risk was losing a
rating — and that's already prevented without matching renames. A moved
directory now just shows up as one new row and one `missing` row; `scan`'s
report lists both so a genuine rename is obvious and fixable by hand. Spec
§5.2, §5.5, §8, §9 and §10 updated to match; see Step 9 below for the revised
reconciliation step.

Quality gates clean: `cargo fmt --all`, `cargo clippy --workspace
--all-targets -- -D warnings`, `cargo test --workspace` (albumdb 40/40,
controller 57/57, album-tool 0 — still a stub).

---

### Step 5 — `albumdb`: queries

- [ ] `upsert_album` — writes derived fields, **never touches user-editable
      columns**. Called for a path whether it's brand new or reappearing
      after `missing` — either way it sets `status = 'present'`.
- [ ] `find_by_path`, `all_present`.
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
- [ ] An album marked `missing` then re-`upsert_album`'d at the same path
      goes back to `status = 'present'` with its user fields untouched.

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

### Step 9 — `scan`: missing-album handling

No automatic rename detection (spec §5.2, §9) — cut after review as
unmeasured complexity. Disappearances are soft-deleted, so the only real risk
was losing a rating, and that's already prevented without matching renames. A
moved directory shows up as one new (unrated) row and one `missing` row that
still carries its rating; reconciling that by hand (repoint the old row's
`path`, drop the duplicate) is a manual SQL job, not a tool feature, for now.

- [ ] Diff MPD paths against stored paths.
- [ ] Path in MPD only → `upsert_album` inserts a new row (Step 8 already
      does this for a first scan; confirm it composes correctly once existing
      rows are in play too).
- [ ] Path in DB only → `mark_missing`. **Never delete a row** — deleting
      silently discards a rating.
- [ ] `scan`'s report lists new and missing paths side by side (spec §5.5),
      so a genuine rename is obvious at a glance.

**Automated:**
- [ ] Remove an album entirely (drop it from a modified fixture) → re-scan →
      `status = 'missing'`, row still present, rating intact.
- [ ] An album returning after being `missing` (same path reappears in the
      fixture) → re-scan → back to `status = 'present'`, rating intact.
- [ ] Renaming a directory in a modified fixture → re-scan → the old path's
      row is `missing` with its rating intact, and a new unrated row exists
      at the new path. Confirms it's *not* silently merged and nothing is
      lost either way.

**Manual:** rename a real album directory, `mpc update`, re-scan, confirm the
old row is `missing` with its rating intact and a new unrated row exists at
the new path. Rename it back, re-scan, confirm it returns to `present` with
the rating.

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
