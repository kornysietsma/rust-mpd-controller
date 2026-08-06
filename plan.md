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

- [x] `crates/album-tool/src/scan.rs`: `group_into_albums(Vec<SongTags>) ->
      Result<Vec<AlbumGroup>>` groups by containing directory
      (`path.rsplit_once('/')`) and derives one `AlbumGroup` (an `AlbumFacts`
      plus its file-tag genre) per album — pure, no I/O, same shape as
      `derive_tag_state`.
- [x] Title/artist resolution per spec §5.3: `Tag` states read `Album`/
      `AlbumArtist` (first non-empty value across the group's tracks —
      relevant only for `mixed_mbid`/`partial_mbid`'s 3 real albums, already
      flagged for manual Picard fixing regardless of which value wins);
      `Path` states go through `parse_album_path`.
- [x] Dates normalised via `normalise_date` (first non-junk value found);
      `label` first non-empty; `track_count` = group size; `disc_count` = max
      parsed `Disc` value, default 1; MB identifiers first non-empty.
- [x] File-tag genre (first non-empty `Genre` across the group) written to
      `album_genres` as `source = 'tag'`, `votes = 0` via `write_albums`,
      which also calls `upsert_album` per group. `replace_genres` with an
      empty list when there's no tag genre keeps a re-scan idempotent.
- [x] `--dry-run`: `main.rs`'s `Scan` handler only opens/writes the database
      when `!dry_run`; the report (album count + `tag_state` distribution,
      spec §5.5) is computed from the pure grouping either way.

**Revised after Korny review, before running against the live library:**
`upsert_album` (Step 5, already "DONE") unconditionally overwrote `title`/
`artist`/`title_source` on every scan, with no guard for spec §5.3's "a row
whose `title_source = 'manual'` is never overwritten by a scan." Fixed in
`albumdb::queries::upsert_album` with a `CASE WHEN title_source = 'manual'
THEN <existing> ELSE <new> END` on all three columns, plus a new test
(`upsert_never_overwrites_a_manual_title`).

**Automated (8 new tests in `scan.rs`, all passing):** full scan of the
fixture into an in-memory DB — `tag_state` distribution matches the fixture's
per-album comments exactly (clean 9, mixed_mbid 1, partial_mbid 1,
no_mbid_consistent 3, no_mbid_varied 3, no_mbid_incomplete 3); `title_source`
is `path` for varied/incomplete and `tag` otherwise; a clean album keeps its
tag title/artist/genre; the multi-disc album gets `disc_count = 2`; a
path-derived album has no tag genre; `'0000'` normalises to no date rather
than a second variation; writing the fixture persists every album and its
tag genre; a second scan of the same fixture changes nothing.

**Bug found and fixed against live MPD, not caught by the fixture:**
`mpd_wire::parse_directory_listing` treated `"playlist" | "Last-Modified"` as
one match arm, both ending the current song. MPD sends `Last-Modified` as the
very next field after every `file:` line, so this silently finalised each
song immediately after `path` was set and before any tag field arrived —
every real scan produced 1,886 albums, all `no_mbid_incomplete`, with every
other column `None`. The committed fixture never exercised this because it
has no `Last-Modified` lines at all — a real gap in Step 7's coverage. Fixed
by dropping `Last-Modified` from that match arm; it now falls through to the
default case and is ignored via `apply_tag`'s catch-all, same as `Added`/
`Format`/`Time`/the `*Sort` fields. Added a regression test
(`last_modified_does_not_terminate_the_current_song`) reproducing the real
field order (`file`, `Last-Modified`, `Added`, `Format`, then tags).

**Manual against live MPD** (`album-tool scan --dry-run`, confirmed by Korny
the library had not changed since Step 7's measurement):

```
scan (dry run, nothing written): 1886 albums
  clean                1833
  mixed_mbid              2
  partial_mbid            1
  no_mbid_consistent     43
  no_mbid_varied          3
  no_mbid_incomplete      4
```

- [x] `clean`/`mixed_mbid`/`partial_mbid` match spec §5.4 exactly.
      `no_mbid_consistent`/`no_mbid_varied`/`no_mbid_incomplete` came out
      43/3/4 rather than the documented 42/5/3 — investigated with a real
      (non-dry-run) scan into a scratch `--db-path` (not the production
      `albums.db`) and raw MPD queries per spec's own AGENTS.md-style
      "check live data" discipline, rather than assumed to be a bug:
    - `Eine Kleine Nachtmusik_Symphonie #29` (Mozart): one `Album`, one
      `AlbumArtist`, `Date` either `1787` or the junk `'0000'` — one real
      year once normalised, so `no_mbid_consistent` is correct per spec
      §5.4's own rule. The original §2.5 measurement predates that rule and
      counted the raw `1787`/`'0000'` mix as two years.
    - `Christmas Music Compilation`: confirmed live that exactly 1 of its
      300 tracks is missing `AlbumArtist` — easy to miss by eye at that
      scale. `no_mbid_incomplete` is correct; §2.5's manual sample missed it.
    - Spec §2.5 and §5.4's table updated to 43/3/4 to match, with the
      Mozart/Christmas explanation recorded there.
- [x] Named cases confirmed live: `Songs for Becky`, `Solid Gold T. Rex [Disc
      1]` and `Hectic City` are `no_mbid_varied`; `Unnmatched/Madness` is
      `no_mbid_incomplete`.
- [x] A second scan changes nothing — proven at the fixture/unit level
      (`a_second_scan_of_the_same_fixture_changes_nothing`); full
      added/updated/missing reconciliation against a real prior scan is
      Step 9.

Quality gates clean: `cargo fmt --all`, `cargo clippy --workspace
--all-targets -- -D warnings`, `cargo test --workspace` (album-tool 36/36,
albumdb 53/53, controller 57/57).

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
