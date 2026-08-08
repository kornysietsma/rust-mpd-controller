# Album Database — Implementation Plan

**Source spec:** [`spec.md`](spec.md)
**Date:** 2026-08-04 (last updated 2026-08-07)

**Status: all 13 build steps are DONE.** What's left is wrap-up only — see
§3. A new session can start directly from §3's checklist; §1 and §2 are
background for anyone extending the tool later (implementing `verify`/
`export`, category derivation, etc. — see spec §9).

---

## 1. Technical context

Durable decisions worth knowing before touching this code again. Day-to-day
crate-API gotchas (what broke, what the fix was) live in `AGENTS.md`, not
here — check there first.

### 1.1 Decisions made when planning

| Decision | Choice | Why |
|---|---|---|
| Test fixtures | Small **synthetic** fixture committed to git (`crates/album-tool/tests/fixtures/library.txt`) | `/album-data` is gitignored and the repo is shared. Invented album names keep the library private, and each awkward case is present deliberately rather than by luck. |
| Real-library verification | `album-tool status` + manual checklist per step | Keeps `cargo test` hermetic and fast. |
| Schema management | Plain `CREATE TABLE IF NOT EXISTS`, changes applied by hand (§1.4) | Single-user tool. `ALTER TABLE` preserves data, so no migration framework earns its keep. |
| Ratings import | Throwaway Python script, no tests (`scripts/import_ratings.py`) | Runs exactly once against real data; done, pending deletion (§3). |
| Clock | Injected `Clock` trait, never a bare `now()` | Needed for `first_seen`/`last_seen`/`rated_at`/`mb_synced_at`. `sync`'s rate limiting uses a separate monotonic `std::time::Instant`-based `RateLimiter` instead — a different concern from wall-clock storage timestamps. |
| Commits | Author (Korny) makes their own | Each step ends with a verification pass, then hand over. |
| MusicBrainz crate | `musicbrainz_rs` 0.13.0 with `async` feature | Fetches release-group genres (with vote counts) and release/media data live. No `reqwest` fallback needed. |

### 1.2 The testability boundary

The single most important structural decision, following a lesson already
learned in this codebase (`AGENTS.md`): **`mpd_client`'s `Song`/`Status` are
`#[non_exhaustive]` with no public constructor, so they cannot be built in a
test.** Same problem with `musicbrainz_rs::Release` (no `Default`, huge
struct).

```
  MPD  ──parse──▶  Vec<SongTags>  ──pure──▶  AlbumFacts  ──▶  albumdb
       (I/O, thin)   plain struct    (all the logic)          (sqlite)
```

- `SongTags` (`crates/albumdb/src/song_tags.rs`) is a plain owned struct with
  named fields, not a `BTreeMap` — the set of tags this project needs is
  fixed and known, so compiler-checked field names beat a string-keyed map.
- Everything interesting — grouping into albums, `tag_state` derivation,
  title/artist resolution, earliest-release-track-count picking — is a
  **pure function over plain domain structs** and is fully testable with
  hand-built values. The MPD/MusicBrainz layers' only job is producing those
  structs; each sits behind a trait so a fixture/fake can substitute for it.

### 1.3 Traits injected

```rust
trait LibrarySource     { fn songs(&self) -> Result<Vec<SongTags>>; }
trait MusicBrainzClient { async fn release_group(&self, mbid: &str) -> Result<ReleaseGroupData, MusicBrainzError>; }
trait Clock              { fn now(&self) -> OffsetDateTime; }
```

Each has three implementations: real, fixture/fake, and (for `Clock`) a fixed
test clock. `ReleaseGroupData`/`MusicBrainzError` are `album-tool`'s own
types (`crates/album-tool/src/musicbrainz.rs`), not `musicbrainz_rs`'s —
keeps that dependency out of the trait's public shape, same reasoning as
`SongTags`.

### 1.4 Schema changes

No migration mechanism, no version marker — deliberate, for a single-user
tool. A schema change is a manual operation: back up the file (or `.dump` it
to SQL), then `ALTER TABLE ADD COLUMN`, or the create/copy/drop/rename
procedure for cases SQLite can't alter in place (e.g. changing a `CHECK`
constraint). Both preserve existing data.

The deferred `verify` columns (`mb_verify_*`) are in the schema from day one,
so adding `verify` later needs no schema change at all.

### 1.5 Error handling

`anyhow::Result` at the CLI boundary. Expected, unsurprising conditions are
**values, not errors**: a MusicBrainz 404 sets `mb_sync_state = 'failed'`
with a reason, it does not abort the run. An unreadable database is a genuine
error and aborts.

### 1.6 Quality gates

No CI in this repo — run by hand before any work is considered done:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Default rustfmt/clippy settings — no config files for either.

---

## 2. Build log (all steps DONE)

Condensed history — what shipped and where, plus decisions not obvious from
the code alone. Crate-API gotchas and bugs already hit (and their fixes) are
in `AGENTS.md`, not repeated here.

1. **Spike** — confirmed `musicbrainz_rs` genre/release fetching and
   `mpd_client` tag coverage live, before writing any real code.
2. **Workspace split** — `src/*.rs` → `crates/controller/src/*.rs` verbatim.
   `controller`/`albumdb`/`album-tool` workspace members. Package renamed
   `rust-mpd-controller` → `controller` (binary filename changed; bundle name
   `MpdController.app` didn't). `install.sh` updated for `cargo bundle -p
   controller`.
3. **`albumdb` schema/connection** — `crates/albumdb/sql/schema.sql` (spec
   §4.2 verbatim, `IF NOT EXISTS`), `crates/albumdb/src/connection.rs`
   (`open`/`open_in_memory`, pragmas applied every open:
   `journal_mode=DELETE`, `foreign_keys=ON`, `busy_timeout=5000`).
4. **Domain types/pure logic** — `crates/albumdb/src/{album,enums,tag_state,
   date,album_path,track_range}.rs`. **Cut after review:** automatic
   rename-matching via a content fingerprint (and the `sha2` dependency it
   needed) — disappearances are already soft-deleted (`status = 'missing'`,
   rating preserved), so the fingerprinting machinery wasn't worth it. A
   moved/renamed directory now just shows up as one new row + one `missing`
   row, fixable by hand (see spec §5.2, §9).
5. **`albumdb` queries** — `crates/albumdb/src/queries.rs`. `upsert_album`'s
   column list and `ON CONFLICT SET` clause structurally omit every
   user-editable column, so it's *impossible* for it to touch them (not just
   "careful not to"). A `CASE WHEN title_source = 'manual'` guard (added in
   Step 8 below) additionally protects a hand-entered title.
6. **`album-tool` skeleton** — **no TOML config.** `db_path`/`port` are
   derived by reading MPD's own `~/.mpd/mpd.conf` off disk
   (`crates/album-tool/src/mpd_conf.rs`) rather than a hand-set config field
   — MPD's protocol `config` command is rejected for non-local clients, but
   reading the file directly sidesteps that since `album-tool` always runs
   on the same machine as MPD. `--mpd-conf`/`--db-path` override each
   independently; `mpd_host` is hardcoded `localhost`.
7. **MPD library reader** — `crates/album-tool/src/{library.rs,mpd_wire.rs}`.
   `mpd_client` has no `base` filter and no typed `find`/`lsinfo`/`listfiles`
   commands, so all three are hand-rolled. Two-level quoting handled in
   `mpd_wire::base_filter`.
8. **`scan`: grouping, `tag_state`, title/artist** —
   `crates/album-tool/src/scan.rs`. Verified against spec §5.4's live counts
   exactly (`clean` 1833 / `mixed_mbid` 2 / `partial_mbid` 1 /
   `no_mbid_consistent` 43 / `no_mbid_varied` 3 / `no_mbid_incomplete` 4).
9. **Missing-album handling** — folded into `scan.rs`/`compute_report`; diffs
   MPD paths against stored paths, `mark_missing` never deletes a row. No
   automatic rename detection (see Step 4 above).
10. **Ratings import** — `scripts/import_ratings.py` (PEP 723 `uv` script,
    stdlib only, no automated tests — one-off by design). Run against the
    real CSV and live `albums.db`: **416 ratings, 69 `is_classical`, 17
    `needs_relisten`**, exact match to spec §2.6. Still committed; deletion
    deferred to the final wrap-up commit (§3).
11. **`status`** — `crates/album-tool/src/status.rs`, read-only report over
    `albumdb::all_albums`.
12. **`sync`: transport** — `crates/album-tool/src/musicbrainz.rs`.
    `musicbrainz_rs`'s own rate limiter is unreachable through any public
    feature flag (`AGENTS.md`), so `album-tool` hand-rolls its own
    `RateLimiter` (`std::time::Instant`, ≥1s between requests) plus
    retry/backoff (`backoff_duration`, ≥3 attempts) and 404 detection. A
    *successful* fetch was deliberately left unpersisted at this step — see
    Step 13.
13. **`sync`: apply results** — `albumdb::set_sync_ok` (one transaction:
    replaces `source = 'mb'` genres, sets `primary_genre` = highest-voted
    genre, `mb_original_count`, `mb_sync_state = 'ok'`; seeds
    `original_tracks = '1-<n>'`/`original_tracks_src = 'mb'` only when
    `original_tracks` is still unset). `mb_original_count` comes from
    `.with_genres().with_releases().with_medias()` on one MusicBrainz
    request — `.with_releases()` alone carries no track-count data,
    confirmed live (`inc=media` is required too; spec §7.1/§7.3 corrected to
    match). Verified against the real library end to end, including a
    genuine process-kill mid-run to prove resumability survives a hard
    interrupt, not just a clean one — see §3 for the final numbers.

---

## 3. Remaining work / final acceptance

This is what a new session should pick up.

**Live library state, as of the last full `sync` run (2026-08-07):**
1,886 present, 0 missing, tag_state distribution exactly matches spec §5.4,
416/69/17 ratings/classical/relisten, **1836 `ok` / 50 `pending` / 0
`failed`** (the 50 permanently-pending are exactly the albums with no MBID
at all — spec §2.1 — so there's nothing left to sync for them), 1829
distinct release groups carrying MB genres (avg 5.3 genres/album, max 29). A
follow-up `scan` was a clean no-op and didn't disturb any of the above.
*Welcome to My Nightmare* spot-checked exactly against spec §2.2's 10 genres.

**Checklist:**

- [x] 1,886 albums, correct `tag_state` distribution.
- [x] 416 ratings, 69 `is_classical`, 17 `needs_relisten`.
- [x] ~1,830 release groups carrying MB genres, several genres each.
- [x] A second `scan` changes nothing.
- [x] No automated command has modified any user-editable field.
- [x] `cargo fmt` / `cargo clippy -D warnings` / `cargo test --workspace` all
      clean (173 tests: album-tool 56, albumdb 60, controller 57).
- [x] No `.db-wal`/`.db-shm` file next to the database.
- [ ] **The menu-bar app still works, installed as a bundle.** Last actually
      confirmed at Step 2 (2026-08-05-ish); not re-verified since, though no
      `controller`-crate code has changed in the meantime. Re-run
      `install.sh`, launch, confirm tray icon/menu/`mpdcontrol://` before
      calling this done.
- [ ] **Delete `scripts/import_ratings.py`** — it has served its one purpose
      (Step 10) and its result is verified in the database; it stays in git
      history. Do this in the same commit as the rest of the wrap-up, not
      earlier.
- [ ] **Final commit** covering all of the above once the two boxes above are
      checked.

---

## 4. Out of scope (deferred, per spec §9)

Schema already has the columns for the first and last of these — no schema
change needed to pick any of this up later:

- `album-tool verify` — re-check tags against current MusicBrainz data, flag
  drift, write `mb_verify_*`.
- `album-tool export` — CSV out.
- All controller/menu-bar features (rating from the menu, showing
  rating/original year in labels, "play a random 5-star album", playing only
  `original_tracks`).
- Category derivation (`is_classical`/`is_spoken`/`is_soundtrack`) from
  MusicBrainz genres.
- Automatic rename reconciliation (content-fingerprint matching of a
  disappeared directory to a newly-appeared one) — cut from v1, see §2 Step
  4. Revisit only if `status`'s missing/new pairs become a repeated
  nuisance, not before.
