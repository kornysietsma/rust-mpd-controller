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
| Clock | **Injected**, never called inline | Standards forbid embedding `now()`. Needed for `first_seen`/`last_seen`/`rated_at`/`mb_synced_at`. (Turned out *not* to cover `sync`'s rate limiting in the end — that wants a monotonic clock, not this wall-clock one; see Step 12.) |
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
trait LibrarySource     { fn songs(&self) -> Result<Vec<SongTags>>; }
trait MusicBrainzClient { async fn release_group(&self, mbid: &str) -> Result<ReleaseGroupData, MusicBrainzError>; }
trait Clock              { fn now(&self) -> OffsetDateTime; }
```

Three implementations each: real, fixture/fake, and (for `Clock`) a fixed test
clock. Nothing else needs mocking. (`MusicBrainzClient::release_group`'s
return type was refined during Step 12 — `ReleaseGroupData`/`MusicBrainzError`
are `album-tool`'s own types, not `musicbrainz_rs`'s, keeping that dependency
out of the trait's public shape.)

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

### Step 5 — `albumdb`: queries — DONE

- [x] `upsert_album` — writes derived fields, **never touches user-editable
      columns**. Called for a path whether it's brand new or reappearing
      after `missing` — either way it sets `status = 'present'`. Implemented
      as a single `INSERT ... ON CONFLICT(path) DO UPDATE` statement whose
      column list and `SET` clause both simply omit every user-editable
      column — the invariant is structural, not a matter of remembering to
      preserve fields.
- [x] `find_by_path`, `all_present`.
- [x] `mark_missing(album_id)` — deliberately does **not** touch `last_seen`
      (that column means "last seen present"; going missing is the opposite
      of that).
- [x] `replace_genres(album_id, source, genres)` — replaces one source, leaves
      the other alone. Runs as a transaction (delete-then-insert).
- [x] User-field setters: `set_rating`, `set_needs_relisten`,
      `set_is_classical`, `set_is_spoken`, `set_is_soundtrack`,
      `set_original_tracks` (always stamps `original_tracks_src = 'manual'`
      — `sync`'s narrower MB-derived write path is its own concern, Step 13),
      `set_match_policy`.

**New files:** `crates/albumdb/src/{album,clock,queries}.rs`, all re-exported
from `lib.rs`.

**New supporting pieces, introduced here as this is the first place time and
full rows enter the picture:**
- `Clock` trait + `SystemClock` + `FixedClock` (plan §1.3) in `clock.rs`,
  backed by the `time` crate (`OffsetDateTime`, RFC 3339 storage format —
  now also spec §4.1). `FixedClock` is public (not `cfg(test)`) since
  `album-tool`'s own tests will need it too in later steps (e.g. Step 12's
  rate-limiting tests).
- `Album` (full row) and `AlbumFacts` (derived-only subset written by
  `upsert_album`) domain structs in `album.rs`. **`AlbumFacts` is the type
  Step 8's `scan` needs to produce** from `Vec<SongTags>` via
  `derive_tag_state`/`parse_album_path`/`normalise_date` before calling
  `upsert_album`.
- Two more schema-backed string enums, following the existing
  `enums.rs`/`string_enum!` pattern: `AlbumStatus` (`present`/`missing`) and
  `GenreSource` (`tag`/`mb`).

**Automated (12 new tests, all passing):**
- [x] **The load-bearing invariant:** set every user-editable column, run
      `upsert_album` with completely different derived data, assert all of
      `rating`, `needs_relisten`, `is_*`, `original_tracks`,
      `original_tracks_src`, `match_policy`, `notes` are unchanged, while
      confirming the derived fields (`title`, `tag_state`, `last_seen`) *did*
      change — proving the invariant isn't vacuously true.
- [x] `replace_genres(.., 'mb', ..)` leaves `source = 'tag'` rows intact, and
      vice versa; re-running with a different set replaces rather than
      duplicates.
- [x] An album marked `missing` then re-`upsert_album`'d at the same path
      goes back to `status = 'present'` with its rating untouched.
- [x] `find_by_path` on an unknown path, `all_present` excluding `missing`
      rows, `set_rating(None, ..)` clearing `rating` but preserving the last
      `rated_at`, `set_original_tracks` stamping `manual`.

Quality gates clean: `cargo fmt --all`, `cargo clippy --workspace
--all-targets -- -D warnings`, `cargo test --workspace` (albumdb 52/52,
controller 57/57, album-tool 0 — still a stub).

---

### Step 6 — `album-tool` skeleton — DONE

**Revised after Korny review:** no TOML config. `album-tool` derives
`db_path` by reading MPD's own `mpd.conf` directly off disk (`music_directory`
+ `port`) rather than asking a config field for it. MPD's protocol `config`
command is rejected for non-local clients (spec §2.7), but `album-tool` always
runs on the same machine as MPD, so reading `mpd.conf` as a plain file
sidesteps that restriction rather than working around it with a hand-set
value. Spec §4.1 and §2.7 updated to match.

- [x] `clap` 4.6.5 derive (`crates/album-tool/src/cli.rs`): `scan`, `sync`,
      `status` as stubs, plus two global flags: `--mpd-conf <path>` (default
      `~/.mpd/mpd.conf`) and `--db-path <path>` (bypasses `mpd.conf`
      derivation entirely, for a scratch/test database).
- [x] `mpd_conf.rs`: a parser for MPD's config-file format (`key "value"` /
      `key value` lines, `#` comments, brace-delimited blocks like
      `audio_output { ... }`) that extracts `music_directory` (required — a
      clear error if absent/commented-out, since it's commented out by
      default in a fresh `mpd.conf`) and `port` (optional, defaults to 6600).
      Brace-depth tracking (clamped at 0, so a stray unmatched `}` can't send
      depth negative and hide real top-level directives afterwards — caught
      by a test, not anticipated up front) means a `port` line nested inside
      an `audio_output` block is never mistaken for the top-level one. Tilde-
      expands `music_directory` (`~/music` is MPD's own documented default
      value) via a pure function taking an explicit home directory, not
      `std::env`. Split into a pure `parse(&str, home: &str) ->
      Result<MpdConf>` and a thin `load(&Path) -> Result<MpdConf>` that reads
      the file, mirroring `controller/src/config.rs`'s `load`/`load_from`
      split for testability.
- [x] `db_path` resolution (`main.rs`): `--db-path` if given, else
      `mpd_conf::load(..).music_directory.join("albums.db")`.
- [x] `mpd_host` is hardcoded `"localhost"`, no flag — reading `mpd.conf` from
      the local filesystem already establishes MPD is local, and
      `bind_to_address` doesn't reliably name a reachable address anyway. Not
      yet used anywhere (no MPD connection until Step 7).

**Automated (15 tests, all passing):** `mpd_conf::parse` — top-level
`music_directory` and `port` parsed; comments and a `port` nested in an
`audio_output` block ignored; tilde expansion; port default when absent;
missing `music_directory` and an invalid `port` value are both clear errors;
a bare stray `}` doesn't hide a later top-level directive (the depth-clamp
bug, caught by this test before it shipped); `load` on a missing file names
`mpd.conf`/`--mpd-conf` in the error. `Cli` parsing smoke tests: `scan
--dry-run`, `sync --limit 5 --force`, `status`, global flags before or after
the subcommand, missing subcommand is an error.

Quality gates clean: `cargo fmt --all`, `cargo clippy --workspace
--all-targets -- -D warnings`, `cargo test --workspace` (album-tool 15/15,
albumdb 52/52, controller 57/57).

**Manual — verified against the real environment:**
- [x] `album-tool --help` and each subcommand's `--help` read sensibly.
- [x] Against the real `~/.mpd/mpd.conf`
      (`music_directory "/Volumes/ssd2024/Dropbox/Dropbox/unsyncd/audio/beets"`),
      `db_path` resolves to
      `/Volumes/ssd2024/Dropbox/Dropbox/unsyncd/audio/beets/albums.db`.
- [x] `--mpd-conf /nonexistent/path` exits 1 with an actionable message, not
      a panic.
- [x] `--db-path /tmp/scratch.db` bypasses `mpd.conf` entirely, as designed.

---

### Step 7 — MPD library reader — DONE

- [x] `LibrarySource` trait (§1.3), deliberately synchronous — the real
      implementation owns a private current-thread `tokio::runtime::Runtime`
      for the call, matching the plan's asymmetric trait signatures (`sync`'s
      `MusicBrainzClient` stays async).
- [x] `MpdLibrarySource` (`crates/album-tool/src/library.rs`): `listfiles` →
      top-level dirs, `lsinfo` per top → chunk boundaries plus loose tagged
      files, `find "(base '<chunk>')"` per chunk, de-duplicated by path.
      `mpd_client`'s `Filter` type has no `base` support, so `find`/`lsinfo`/
      `listfiles` are hand-rolled `Command` impls (AGENTS.md).
- [x] Two-level quoting helper (`mpd_wire::base_filter`, spec §2.7).
- [x] `FixtureLibrarySource` (test-only) reading the committed
      `crates/album-tool/tests/fixtures/library.txt` — 20 invented albums (44
      songs) covering all six `tag_state` values, `Date '0000'`, a multi-disc
      album, a missing `AlbumArtist`, an apostrophe path, and all four
      `parse_album_path` patterns. This fixture is reused by Steps 8–9.
- [x] `crates/album-tool/src/mpd_wire.rs`: the shared pure parser
      (`parse_directory_listing`) used by both the real MPD commands and the
      fixture loader, since the fixture file uses the same `key: value` line
      shape MPD's protocol does.

**Automated:** 19 new tests (album-tool 27/27, albumdb 52/52, controller
57/57 — all quality gates clean).

**Manual against live MPD:** `album-tool scan --dry-run` (wired
provisionally into the `Scan` handler — reads and counts songs; grouping and
writing are Step 8) reads exactly **29,028** songs across **1,886** distinct
directories, matching spec §2.1 and MPD's own `stats` output. Caught and
fixed one real bug this way — `listfiles`'s `file` entries are untagged
filesystem noise, not songs (spec §2.7, AGENTS.md).

**Spec updated:** §2.7 gained the `listfiles` finding; §4.1 clarified that
`--db-path` bypasses only database-path derivation — `mpd.conf` is still read
for the port whenever a command needs to reach MPD.

---

### Step 8 — `scan`: grouping, `tag_state`, title/artist — DONE

No reconciliation yet (Step 9). Every scan is treated as a first scan.

- [x] `crates/album-tool/src/scan.rs` (new): `group_into_albums(Vec<SongTags>)
      -> Result<Vec<AlbumGroup>>` groups by containing directory and derives
      one `AlbumGroup` (an `AlbumFacts` plus its file-tag genre) per album —
      pure, no I/O, same shape as `derive_tag_state`. `write_albums` calls
      `upsert_album` + `replace_genres` per group. Where a group's tracks
      disagree on a scalar field (only possible for `mixed_mbid`/
      `partial_mbid`), the first non-empty value in path-sorted order wins —
      those states are already flagged for manual Picard fixing regardless.
- [x] `main.rs`'s `Scan` handler: `--dry-run` skips opening/writing the
      database; the report (album count + `tag_state` distribution, spec
      §5.5) is computed from the pure grouping either way.

**Revised after Korny review:** `upsert_album` (Step 5, already "DONE") had
no guard for spec §5.3's "a row whose `title_source = 'manual'` is never
overwritten by a scan." Fixed with a `CASE WHEN title_source = 'manual'`
guard on `title`/`artist`/`title_source` in the `ON CONFLICT` clause.

**Bug found and fixed against live MPD, not caught by the fixture:** every
real scan was landing all 1,886 albums as `no_mbid_incomplete` with every tag
field `None`. Root cause and fix are in `AGENTS.md` (`mpd_wire` /
`Last-Modified` gotcha) — worth reading before touching `mpd_wire.rs` again.

**Numbers vs. spec §5.4:** live counts are `clean` 1833 / `mixed_mbid` 2 /
`partial_mbid` 1 (exact match) and `no_mbid_consistent` 43 / `no_mbid_varied`
3 / `no_mbid_incomplete` 4 (spec originally said 42/5/3). Investigated live,
confirmed not a bug, and spec §2.5/§5.4 updated to match — see spec.md for
the explanation (a Mozart album's junk-year normalisation, and one blank
`AlbumArtist` among 300 tracks in a Christmas compilation).

**Automated:** 8 new tests in `scan.rs` (fixture `tag_state` distribution,
title_source per state, tag-genre handling, multi-disc `disc_count`, date
normalisation, idempotent re-scan) plus a `mpd_wire.rs` regression test for
the `Last-Modified` bug and an `upsert_album` test for the manual-title
guard. `cargo test --workspace`: album-tool 36/36, albumdb 53/53, controller
57/57. `cargo fmt --all` / `cargo clippy --workspace --all-targets -- -D
warnings` clean.

---

### Step 9 — `scan`: missing-album handling — DONE (automated)

No automatic rename detection (spec §5.2, §9) — cut after review as
unmeasured complexity. Disappearances are soft-deleted, so the only real risk
was losing a rating, and that's already prevented without matching renames. A
moved directory shows up as one new (unrated) row and one `missing` row that
still carries its rating; reconciling that by hand (repoint the old row's
`path`, drop the duplicate) is a manual SQL job, not a tool feature, for now.

- [x] Diff MPD paths against stored paths — `scan::compute_report`
      (`crates/album-tool/src/scan.rs`) reads `all_present` plus one
      `find_by_path` per group. Entirely read-only, so it also backs
      `--dry-run`.
- [x] Path in MPD only → `upsert_album` inserts a new row (Step 8 already
      did this for a first scan; the reappearance test below confirms it
      still composes correctly once existing rows are in play).
- [x] Path in DB only (previously `present`) → `mark_missing`. Never deletes
      a row.
- [x] `scan`'s report lists new and missing paths side by side (spec §5.5):
      `ScanReport { added, updated, newly_missing }`, printed by both `scan`
      and `scan --dry-run` — the dry-run path now opens the database
      read-only to compute the diff, rather than skipping it entirely as
      Step 8's placeholder did.

**Automated (3 new tests, all passing):**
- [x] Removing an album from the fixture → re-scan → `status = 'missing'`,
      rating intact, `last_seen` untouched (it records "last seen present").
- [x] A `missing` album reappearing (same path back in the fixture) →
      re-scan → back to `present`, rating intact, `last_seen` updated.
- [x] Renaming a directory in the fixture → re-scan → old path `missing`
      with its rating intact, new path a fresh unrated row — confirms it's
      *not* silently merged, matching the "no auto-rename" decision above.

Quality gates clean: `cargo fmt --all`, `cargo clippy --workspace
--all-targets -- -D warnings`, `cargo test --workspace` (album-tool 39/39,
albumdb 53/53, controller 57/57).

**Manual verification deferred, not done.** Checked `--dry-run` against the
real `mpd.conf`-derived `albums.db` and found it has **zero rows** — Step 8's
"Manual" paragraph recorded live counts matching spec §5.4, but nothing was
left persisted (either that check ran read-only, or the file was reset
since). Not a Step 9 bug: against an empty DB, `--dry-run` correctly reported
all 1,886 albums as `added`, 0 `updated`, 0 `missing`. Korny chose to skip a
throwaway real scan just to exercise-and-revert a directory rename, and fold
Step 9's live rename check into Step 10's first real scan instead.

---

### Step 10 — Ratings import (Python one-off, not Rust) — DONE

Ran a real `album-tool scan` (not `--dry-run`) against the live library —
was empty going into this step (Step 9's last paragraph). Result: 1,886
albums written, `tag_state` distribution an exact match to spec §5.4
(1833/2/1/43/3/4), idempotent on a second run (`0 added / 1886 updated / 0
missing`). Also exercised the deferred Step 9 rename check live (rename a
real directory, `mpc update --wait`, rescan, rename back, rescan): confirmed
old-path-`missing`/new-path-added and rename-back-to-`present`-as-`updated`
both work exactly as designed (§5.2, no auto-rename matching).

**Unrelated to `album-tool` itself, found and fixed while exercising `mpc
update` live:** two pre-existing Dropbox `(Case Conflict)` directory splits
in the real library (`Steve Earle & The Dukes`, `Charli XCX`), caused by
case-inconsistent MusicBrainz artist credits between releases. Fixed by hand
(tag + directory correction on both) and added a standing beets `inline`
plugin workaround so future imports reuse existing on-disk casing instead of
creating a new split — documented outside this repo at
`~/.config/beets/case_sensitive_beets_workaround.md`. Not this project's
concern beyond the fact that it's why the live database briefly showed extra
`missing` rows mid-step; both are cleaned up now.

`scripts/import_ratings.py` (PEP 723 `uv` script, stdlib only, no Rust, no
automated tests — spec §6) written and run against the real CSV and the live
`albums.db`. Unmatched paths are handled by a preflight check that aborts
before writing anything rather than silently skipping (none triggered — spec
§2.6 predicted this). Result, verified both in the script's own summary and
by direct SQL: **416 ratings, 69 `is_classical`, 17 `needs_relisten`** —
exact match to spec. Re-ran `scan` afterwards and confirmed all three counts
survive untouched, proving the §5 no-user-field-overwritten invariant holds
against real data, not just the fixture. `ruff check`/`format` clean.

**State the live `albums.db` is now in, for Step 11 onward:** 1,886
`present` / 0 `missing`, tag_state distribution per spec §5.4, 416/69/17
ratings set. No longer empty — `status` and `sync` steps run against this
real data from here on.

Quality gates clean; counts unchanged from Step 9 since this step added no
Rust code (album-tool 39/39, albumdb 53/53, controller 57/57).

**Not yet done:** `scripts/import_ratings.py` is still committed. Its
deletion is deferred to the final commit in §3, after `sync` (Steps 12–13)
is also complete.

---

### Step 11 — `status` — DONE

- [x] Counts by `tag_state` and `mb_sync_state`.
- [x] Albums with `status = 'missing'`.
- [x] `mixed_mbid` / `partial_mbid` albums listed **for manual fixing in
      Picard** — the tool reports, it never edits files.
- [x] Failed syncs with their errors.
- [x] Rating coverage and `needs_relisten` count.

**New:** `crates/album-tool/src/status.rs` —
`compute_status_report(&Connection) -> StatusReport`, a pure read over
`albumdb::all_albums` (new query, `all_present`'s counterpart that includes
`missing` rows too). `TagStateCounts` (from `scan.rs`) gained a
`from_tag_states` constructor so `status` reuses its six-arm match instead of
duplicating it; `SyncStateCounts` is the analogous new type for
`mb_sync_state`. `Display` renders in the same indented-text style as
`scan`'s report.

**Automated:** 3 new album-tool tests (seeded in-memory DB → every report
section at once, plus an all-missing DB), 1 new albumdb test (`all_albums`).
Quality gates clean (album-tool 41/41, albumdb 54/54, controller 57/57).

**Manual — verified against the live database:** output matches spec §8
exactly, including both real `mixed_mbid` albums and the one `partial_mbid`
album (`Billy Bragg/The Million Things That Never Happened`, `Dire
Straits/Brothers in Arms`, `Unnmatched/rowan_8th_birthday`). `mb_sync_state`
is all `pending`, as expected before `sync` exists.

---

### Step 12 — `sync`: transport — DONE

Transport and discipline only; persistence (writing genres and marking a row
`ok`) is Step 13.

- [x] `MusicBrainzClient` trait (`crates/album-tool/src/musicbrainz.rs`) plus
      `RealMusicBrainzClient` wrapping `musicbrainz_rs` 0.13.0. Returns a
      minimal own domain type (`ReleaseGroupData { genres: Vec<GenreVote> }`),
      not the raw `musicbrainz_rs::ReleaseGroup` — same
      not-leaking-the-dependency shape as `SongTags` for MPD. `FakeMusicBrainzClient`
      (`#[cfg(test)]`, queues per-MBID response sequences) backs every test below.
- [x] Rate limit **≥1 s between requests** — hand-rolled (`RateLimiter`,
      `std::time::Instant`), **not** the injected wall-clock `Clock`.
      `musicbrainz_rs`'s own built-in limiter turned out to be unreachable
      through any public feature flag — see `AGENTS.md` for the finding, and
      spec §7.1/§4.1 for the updated design text. Tests use real short
      durations (Korny's call) rather than a fake-clock/Sleeper abstraction.
- [x] Descriptive User-Agent: `album-tool/0.1.0 (korny@sietsma.com)`
      (`musicbrainz::USER_AGENT`).
- [x] Retry `503`/`5xx` with exponential backoff (`backoff_duration`, pure,
      unit-tested), ≥3 attempts (`RequestPolicy::max_attempts`, default 3) —
      the crate retries a bare `503` internally already; this wraps the whole
      call for everything else it doesn't cover (`AGENTS.md`).
- [x] `404` → detected via `err.as_musicbrainz_error().is_some_and(|e|
      e.is_not_found())`, no retry, `mb_sync_state = 'failed'` via the new
      `albumdb::set_sync_failed`; the run continues.
- [x] Give-up after `max_attempts` → `failed` the same way. Never leaves a
      row `pending` after a real attempt.
- [x] Commit state per album: `set_sync_failed` is one `UPDATE` per album as
      each release-group result comes back, not batched — an interrupted
      run's earlier failures are already durable.

**Scope boundary vs. Step 13 (Korny's call):** a *successful* fetch is **not
persisted** here — the row stays `mb_sync_state = 'pending'`, and
`SyncReport` says so explicitly ("fetched ok, not yet persisted"). Only
failures write to the database. Step 13 must write genres + `mb_sync_state =
'ok'` together, atomically, so a row is never `ok` without the genre data
that justifies it — **when Step 13 runs, it will re-fetch every row Step 12
already fetched successfully**, since nothing about that fetch was saved.

**New/changed:** `albumdb::pending_sync_albums` (spec §7.1's selection,
`--force` widens `mb_sync_state = 'pending'` to also include `ok`/`failed`)
and `albumdb::set_sync_failed`. `album-tool::sync::run_sync` selects, groups
by `mb_release_group_id` (dedup), applies `--limit` to the deduplicated group
count, and loops with the rate limiter between requests. Wired into `main.rs`'s
`Sync` handler via a private `tokio` runtime (same pattern as `MpdLibrarySource`).

**Automated:** 12 new tests (album-tool 53/53, albumdb 57/57, controller
57/57), covering backoff math, rate-limiter timing (real ~30ms spacing
assertion), selection (`match_policy = 'never'`, dedup by release group,
`--limit`, `--force` vs. not), and outcome handling (success leaves `pending`,
404 and give-up both record `failed` and the run continues, a prior failure
isn't re-attempted without `--force`). Quality gates clean (`fmt`, `clippy -D
warnings`, `test --workspace`).

**Manual against the live API:** `album-tool sync --limit 1` and `--limit 3`
against the real `mpd.conf`-derived `albums.db` both succeeded; the 3-request
run took ~3.8s wall time, confirming the ≥1s rate limit is actually enforced
against a real network round trip.

---

### Step 13 — `sync`: apply results

**Continuing from Step 12:** the hook point is `sync::run_sync`'s `Ok(_data)
=>` arm (`crates/album-tool/src/sync.rs`) — currently just counts
`albums_fetched_ok` and discards `_data`; extend it to persist instead. A
success there means a **fresh** fetch, since Step 12 never saved one — no
special-casing needed for rows that "already succeeded" under Step 12.
`musicbrainz::ReleaseGroupData` (`crates/album-tool/src/musicbrainz.rs`)
currently only carries `genres: Vec<GenreVote>`; add
`earliest_release_track_count: Option<i64>` (or similar) and have
`RealMusicBrainzClient::release_group` add `.with_releases()` alongside the
existing `.with_genres()` on the same request — no second round trip needed
(spec §7.3, confirmed by the Step 1 spike). `albumdb` needs a new `set_sync_ok`
(or extend `replace_genres`'s call site) that writes `mb_sync_state = 'ok'`,
`mb_synced_at`, `primary_genre`, `mb_original_count` and calls
`replace_genres(.., GenreSource::Mb, ..)` together — ideally in one
transaction, so a row is never observably `ok` without its genre data.

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
