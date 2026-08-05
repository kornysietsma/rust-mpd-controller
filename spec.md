# Album Database — Technical Specification

**Status:** ready for implementation
**Date:** 2026-08-04
**Supersedes:** `album-data/albums_and_musicbrainz_prompt.md` (the original brief)

---

## 1. Summary

Build a SQLite album database covering every album in the MPD library, holding
metadata derived from file tags, enrichment from the MusicBrainz API, and
user-editable fields (ratings, category flags, original-track ranges).

The database is consumed by hand (SQL), by agent skills, and — later — by the
`rust-mpd-controller` menu-bar app.

Delivered as a Cargo workspace with a new `album-tool` CLI. **The menu-bar app
is not modified in v1** beyond relocating into the workspace.

---

## 2. Findings that shaped this design

Everything below was measured against the live MPD 0.24.0 server on
`localhost:6600` on 2026-08-04. These findings materially changed the design
from the original brief and should be re-checked if the library changes
substantially.

### 2.1 The library is already well tagged

| Signal | Coverage (of 1,886 album directories) |
|---|---|
| Clean single MusicBrainz release ID | 1,833 |
| No MBID at all | 50 |
| Mixed (>1 release ID in one directory) | 2 |
| Partial (some tracks lack an MBID) | 1 |
| `OriginalDate` on every track | 1,880 |
| `MUSICBRAINZ_RELEASEGROUPID` on every track | 1,835 |
| `Genre` on every track | 1,784 (78 distinct values) |
| Multi-disc directories | 195 |

Library totals: 29,028 songs, 1,886 album directories, across three top-level
directories — `Albums/` (1,663), `Compilations/` (196), `Unnmatched/` (27).

**Consequence:** original release dates and a usable genre are already present
in the files. `OriginalDate` differs from `Date` in **421** directories, and in
three sampled albums it matched the MusicBrainz release-group
`first-release-date` *exactly*. The brief assumed MusicBrainz was the primary
source for dates; it is not. Tags are.

### 2.2 MusicBrainz is still needed — for genres

Every tagged track carries **exactly one** genre. Measured: 27,250 tracks with
one genre, 1,778 with none, **zero with more than one**.

This is *not* an MP3 format limitation. ID3v1's fixed 148-genre list has been
obsolete for decades; ID3v2.4 stores free text and multiple values, as the
library itself demonstrates (`Post-Punk`, `Synthpop`, `Dark Cabaret` are not
ID3v1 entries). The single genre per track reflects Picard's default
configuration.

MusicBrainz returns roughly 7–10 **vote-weighted** genres instead:

| Album | File tag | MusicBrainz release-group genres |
|---|---|---|
| Welcome to My Nightmare | `Hard Rock` | hard rock(9), dark cabaret(5), rock(5), classic rock(3), glam rock(3), art rock(2), heavy metal(2), pop rock(2), rock opera(2), vaudeville(2) |
| Funeral | `Indie Rock` | indie rock(17), chamber pop(4), art pop(2), alternative punk(1), art rock(1), baroque pop(1), rock(1), symphonic rock(1) |
| Brothers in Arms | `Rock` | rock(20), classic rock(8), pop rock(6), blues rock(4), heartland rock(2), soft rock(2), country rock(1) |

Two critical details:

- **Genres live on the release group, not the release.** All three sampled
  releases returned zero release-level genres. Query
  `/ws/2/release-group/<mbid>?inc=genres`.
- **Use `inc=genres`, not `inc=tags`.** The `tags` endpoint mixes in
  folksonomy noise — `5+ wochen`, `english`, `ph_5_stars`, moods, years.

### 2.3 A full sync is fast

**1,830 distinct release-group MBIDs → ~31 minutes** at 1 request/second.

The brief anticipated "slow-running tasks over multiple hours". It is one
coffee break. The sync must be *resumable*, but it does not need an elaborate
multi-session job scheduler.

One `503 Service Temporarily Unavailable` occurred in four sample requests, so
**retry with backoff is mandatory, not optional.**

### 2.4 MPD exposes every tag needed — no MP3 parsing required

MPD surfaces `Album`, `AlbumArtist`, `Artist`, `Title`, `Track`, `Date`,
`OriginalDate`, `Genre`, `Label`, `Disc`, `Composer` and all six MusicBrainz ID
tags directly. **No `id3`/`lofty` crate is needed anywhere in this project.**
This eliminates the "MP3 tags are a headache" concern from the brief entirely.

### 2.5 The unmatched directories split three ways

Of the 50 directories with no MBID:

- **42 are consistent** — one `Album`, one `AlbumArtist`, one year. Artists
  vary per track, but that is a normal compilation with
  `AlbumArtist=Various Artists`. **These tags are usable at album level.**
- **5 are genuinely varied** — `Unnmatched/Various Artists/Songs for Becky`
  (19 artists, 14 distinct years), `Christmas Music Compilation` (2 album
  titles, 300 artists, 42 distinct years), `Solid Gold T. Rex [Disc 1]`,
  `The Kleptones/Hectic City`, `Eine Kleine Nachtmusik_Symphonie #29`.
- **3 have partial tags** — `Unnmatched/Madness` (no `Album`),
  `Neil Gaiman/BBC Radio 4 - Neverwhere` and `Traditional/Activity Songs`
  (no `AlbumArtist`).

**Artist variation is a false signal** for detecting hand compilations —
compilations legitimately have many artists under one `AlbumArtist`. The real
signals are **`Album` title variation and `Date` variation**.

`0000` appears as a junk year (`Becky Korny Wedding Edits`; mixed with `1787`
on the Mozart) and must be normalised to NULL.

### 2.6 The ratings CSV is clean

`album-data/all_albums_export.csv`, 1,848 rows:

| Value | Count | Meaning |
|---|---|---|
| *(blank)* | 1,346 | not rated |
| `5` | 228 | rating 5 |
| `4` | 187 | rating 4 |
| `c` | 69 | classical |
| `?` | 17 | can't rate yet |
| `0` | 1 | rating 0 |

- **All 1,848 CSV paths still exist in MPD.** Zero missing. The import is safe.
- 39 MPD directories are newer albums absent from the CSV.
- One duplicate path: `Compilations/Sound Environment` appears twice.
- **There are no 1s, 2s or 3s.** The nominal 0–5 scale is used in practice as
  good / great / unrated. The schema keeps the full range anyway.
- The CSV's 69 `c` marks are a **better source of truth for classical** than
  file tags, which yield only 35 (`Classical`+`Opera`+`Baroque`+`Orchestra`+
  `Contemporary Classical`) — many classical albums are among the 101
  directories with no genre tag.

### 2.7 MPD protocol gotchas encountered

Recorded because they cost time and will recur:

- **`find "(base '')"` over the whole library returns zero rows.** The response
  is too large. Chunk by second-level directory (`Albums/A`, `Albums/B`,
  `Compilations/…`) — `count "(base '')"` works fine and returns 29,028.
- **`lsinfo` and `listallinfo` with no argument return zero rows** and then
  drop the connection. Use `listfiles` to enumerate top-level directories.
- **Filter arguments need two levels of quoting.** The filter is escaped for
  its inner single-quoted string, then the whole filter is escaped again as
  the outer double-quoted protocol argument. Paths like
  `A Wake-Up Call for the '90s` fail otherwise.
- **`config` is rejected** for non-local clients, so the music directory cannot
  be discovered over TCP — it must be configured.

---

## 3. Architecture

### 3.1 Workspace layout

```
rust-mpd-controller/
├── Cargo.toml              # [workspace]
├── crates/
│   ├── albumdb/            # schema, queries, domain types
│   ├── controller/         # existing menu-bar app, moved verbatim
│   └── album-tool/         # CLI: scan / sync / status
└── docs/
```

**Rationale:** the menu-bar app must not acquire the MusicBrainz HTTP stack in
its dependency tree. A workspace keeps `controller` depending only on the thin
`albumdb` crate, while `album-tool` carries `musicbrainz_rs`, `reqwest` and
`clap`.

**Migration of the existing app is a pure move.** `src/*.rs` →
`crates/controller/src/*.rs`, with `[package.metadata.bundle]` and the
`osx_url_schemes` config following it. No code changes. This must be verified
by building and running the app before any new work starts.

### 3.2 Crate responsibilities

| Crate | Depends on | Responsibility |
|---|---|---|
| `albumdb` | `rusqlite`, `serde`, `anyhow` | Schema, all SQL, domain types (`Album`, `TagState`, `Rating`), track-range parsing. No network, no MPD. |
| `controller` | `albumdb` + existing deps | Unchanged in v1. |
| `album-tool` | `albumdb`, `clap`, `musicbrainz_rs`, `tokio`, `mpd_client` | CLI. Talks to MPD and MusicBrainz, writes via `albumdb`. |

### 3.3 Dependency versions

Current stable as of 2026-08-04:

- `rusqlite` 0.40.1 (with `bundled` feature)
- `clap` 4.6.5 (with `derive`)
- `musicbrainz_rs` 0.13.0, **with the `async` feature enabled** (off by
  default)

**Confirmed by spike (2026-08-05):** `musicbrainz_rs` fetches release-group
genres with vote counts — `ReleaseGroup::fetch().with_genres()` populates
`genres: Option<Vec<Genre>>`, and `Genre.count` is the vote count. Live-fetched
against the real API and got exactly the numbers in §2.2. `.with_releases()`
also works, covering §7.3's `mb_original_count`. No `reqwest` fallback is
needed. Usage gotchas (feature flag, client construction, built-in rate
limiter) are recorded in `AGENTS.md`.

---

## 4. Database

### 4.1 Location and pragmas

`albums.db` lives **in the music folder**, so it is backed up and moved with the
music.

The music folder is synced to Dropbox as backup and copied to a NAS over SMB,
but is **never** written concurrently from two machines. Multi-writer safety is
therefore a non-goal.

One residual risk does need handling: with WAL enabled, Dropbox can upload
`albums.db` and `albums.db-wal` at different moments and capture an
inconsistent pair — corrupting the *backup* even though the live file is fine.

```sql
PRAGMA journal_mode = DELETE;   -- no WAL sidecar; .db always self-consistent
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
```

`journal_mode=DELETE` costs nothing at this scale (1,886 rows, batch writes)
and makes every Dropbox snapshot restorable.

The database path comes from the `album-tool` config (TOML, sharing
`config.rs` conventions with the controller) with no default — it must be set
explicitly, since MPD's `music_directory` is not discoverable over TCP (§2.7).

### 4.2 Schema

```sql
CREATE TABLE albums (
    album_id            INTEGER PRIMARY KEY,
    path                TEXT    NOT NULL UNIQUE,   -- 'Albums/E/EELS/EELS TIME!'

    -- identity / presence
    status              TEXT    NOT NULL DEFAULT 'present',  -- present | missing
    fingerprint         TEXT,                       -- for rename detection, §5.2
    first_seen          TEXT    NOT NULL,
    last_seen           TEXT    NOT NULL,

    -- descriptive, from tags or path (§5.3)
    title               TEXT,
    artist              TEXT,                       -- AlbumArtist
    title_source        TEXT,                       -- tag | path | manual
    date                TEXT,                       -- release date of this edition
    original_date       TEXT,                       -- OriginalDate; '0000' -> NULL
    label               TEXT,
    track_count         INTEGER NOT NULL,
    disc_count          INTEGER NOT NULL DEFAULT 1,

    -- MusicBrainz identifiers, from file tags
    mb_release_id       TEXT,
    mb_release_group_id TEXT,
    mb_albumartist_id   TEXT,

    -- derived every scan, never hand-edited (§5.4)
    tag_state           TEXT    NOT NULL,

    -- written only by `sync`
    mb_sync_state       TEXT    NOT NULL DEFAULT 'pending', -- pending|ok|failed|skipped
    mb_synced_at        TEXT,
    mb_sync_error       TEXT,
    mb_original_count   INTEGER,        -- track count of earliest release in group
    primary_genre       TEXT,           -- cached top-voted MB genre

    -- reserved for the deferred `verify` command (§8)
    mb_verify_state     TEXT    NOT NULL DEFAULT 'unverified', -- unverified|ok|drifted
    mb_verified_at      TEXT,
    mb_verify_notes     TEXT,

    -- user-editable; never machine-written
    rating              INTEGER,
    needs_relisten      INTEGER NOT NULL DEFAULT 0,
    rated_at            TEXT,
    is_classical        INTEGER NOT NULL DEFAULT 0,
    is_spoken           INTEGER NOT NULL DEFAULT 0,
    is_soundtrack       INTEGER NOT NULL DEFAULT 0,
    original_tracks     TEXT,           -- '1-10' | '1-12,14' | NULL = all
    original_tracks_src TEXT,           -- manual | mb
    match_policy        TEXT    NOT NULL DEFAULT 'auto',  -- auto | never | recheck
    notes               TEXT,

    CHECK (rating IS NULL OR rating BETWEEN 0 AND 5),
    CHECK (status IN ('present','missing')),
    CHECK (match_policy IN ('auto','never','recheck')),
    CHECK (tag_state IN ('clean','mixed_mbid','partial_mbid',
                         'no_mbid_consistent','no_mbid_varied','no_mbid_incomplete'))
);

CREATE INDEX idx_albums_sync    ON albums(mb_sync_state) WHERE status = 'present';
CREATE INDEX idx_albums_rg      ON albums(mb_release_group_id);
CREATE INDEX idx_albums_rating  ON albums(rating);

CREATE TABLE album_genres (
    album_id   INTEGER NOT NULL REFERENCES albums(album_id) ON DELETE CASCADE,
    genre      TEXT    NOT NULL,      -- 'dark cabaret'
    votes      INTEGER NOT NULL DEFAULT 0,
    source     TEXT    NOT NULL,      -- 'mb' | 'tag'
    PRIMARY KEY (album_id, genre, source)
);

CREATE INDEX idx_genres_genre ON album_genres(genre);
```

All statements use `IF NOT EXISTS`. There is deliberately **no migration
mechanism and no version marker** — this is a single-user tool, so schema
changes are applied by hand: back up the file (or `.dump` it to SQL), then
`ALTER TABLE ADD COLUMN`, or the standard create/copy/drop/rename procedure for
the rarer cases SQLite can't alter in place, such as changing a `CHECK`
constraint. Both preserve existing data, so no hand-entered value is ever at
risk.

A stored version number is not worth it here: a hand-applied change would leave
it stale, and a stale marker is worse than none.

**Design notes**

- **Album-level only — there is no tracks table.** Anything track-shaped is
  queried live from MPD. `track_count` and `disc_count` are cached because
  `status` reporting and fingerprinting need them.
- **Three independent state concerns**, each changed for its own reasons and
  unable to corrupt the others: `tag_state` (derived, rewritten every scan),
  the `mb_sync_*` / `mb_verify_*` groups (written only by their own command),
  and `match_policy` (yours, never machine-written).
- **`match_policy = 'never'`** means no automated pass will ever attempt a
  MusicBrainz match. This is the setting for `Songs for Becky`,
  `rowan_8th_birthday` and similar — the directory wins permanently.
- **Category is three boolean flags, not an enum.** ~90% of the library would
  land in a single "popular" bucket, so the value here is *exclusion*
  (`WHERE NOT is_classical AND NOT is_spoken AND rating >= 4`), and flags
  handle overlap — a classical soundtrack is legitimately both.
- **`needs_relisten` is orthogonal to `rating`**, so it also expresses "rated 4
  years ago, no longer trust it" — which a sentinel value in `rating` cannot.
- **Every user-editable column survives every automated pass.** This is the
  single most important invariant in the schema.

---

## 5. `album-tool scan`

```
album-tool scan [--dry-run]
```

Reads MPD, writes `albums`. Idempotent. No network beyond MPD.

### 5.1 Reading the library

1. `listfiles` → top-level directories (`Albums`, `Compilations`, `Unnmatched`).
2. `lsinfo "<top>"` → second-level directories, used as chunk boundaries.
3. `find "(base '<chunk>')"` per chunk, honouring the two-level quoting rule
   (§2.7). Also query each top-level directory itself to catch loose files, and
   de-duplicate by file path.
4. Group songs by `path.rsplit('/', 1)[0]` — the containing directory is the
   album. Multi-disc albums are a single directory with continuous track
   numbering; there are no `CD1/`/`CD2/` subdirectories.

### 5.2 Reconciling against existing rows

- Path present in both MPD and DB → update derived fields.
- Path in MPD only → candidate new album.
- Path in DB only → candidate disappearance.

For each candidate disappearance, look for a candidate new album with a
matching **fingerprint**; if found, treat it as a rename and `UPDATE` the
existing row's `path`, preserving `album_id` and every user-editable field.

The fingerprint is a stable hash over: track count, the sorted set of track
titles, and the sorted set of distinct `MUSICBRAINZ_TRACKID` values (empty for
untagged albums, in which case titles carry the match).

Unmatched disappearances are set to `status = 'missing'` and **never deleted** —
deleting would silently discard a rating. `status` reports them.

### 5.3 Populating title and artist

Tags are used when trustworthy, the directory path otherwise, and
`title_source` always records which:

| `tag_state` | Source | `title_source` |
|---|---|---|
| `clean`, `mixed_mbid`, `partial_mbid`, `no_mbid_consistent` | tags | `tag` |
| `no_mbid_varied`, `no_mbid_incomplete` | directory path | `path` |

Path parsing:

| Pattern | title | artist |
|---|---|---|
| `Albums/<letter>/<artist>/<album>` | `<album>` | `<artist>` |
| `Compilations/<album>` | `<album>` | `Various Artists` |
| `Unnmatched/<artist>/<album>` | `<album>` | `<artist>` |
| `Unnmatched/<album>` | `<album>` | NULL |

A row whose `title_source = 'manual'` is never overwritten by a scan.

**`Date` and `OriginalDate` of `0000` normalise to NULL** everywhere.

The file-tag genre is written to `album_genres` with `source = 'tag'` and
`votes = 0`.

### 5.4 Deriving `tag_state`

Computed per directory, rewritten on every scan:

| State | Condition | Expected count |
|---|---|---|
| `clean` | every track has an MBID, and all are the same | 1,833 |
| `mixed_mbid` | every track has an MBID, but >1 distinct value | 2 |
| `partial_mbid` | some tracks have an MBID, some don't | 1 |
| `no_mbid_consistent` | no MBIDs; one `Album`, one `AlbumArtist`, one year | 42 |
| `no_mbid_varied` | no MBIDs; `Album` title or year varies across tracks | 5 |
| `no_mbid_incomplete` | no MBIDs; `Album` or `AlbumArtist` absent | 3 |

**Artist variation is explicitly not a signal** (§2.5). Year comparison uses
the first four characters of `Date`, ignoring `0000`.

Because `tag_state` is recomputed from scratch every scan, an album that gains
MusicBrainz tags (hand-tagged in Picard) transitions `no_mbid_* → clean`
automatically, and `sync` picks it up on the next run. **This satisfies the
brief's "identify if they have become taggable" requirement with no extra
mechanism.**

### 5.5 Output

Counts of albums added, renamed, updated, and newly missing; the `tag_state`
distribution; and a list of anything needing attention. `--dry-run` reports
without writing.

---

## 6. Ratings import — a one-off script, not part of the tool

**This is explicitly not a Rust subcommand.** It runs exactly once against real
data, so it is a throwaway Python script (`uv`, PEP 723 inline metadata) at
`scripts/import_ratings.py`, and carries no automated tests. Correctness is
established by inspecting the resulting data, not by a test suite.

It is committed to git for posterity and deleted once the work is complete —
history keeps the record. It takes the CSV and database paths as arguments, so
that nothing under the gitignored `album-data/` is baked into a tracked file.

Only the `rating` column of `all_albums_export.csv` is imported — artist, album
and date all come from tags or MusicBrainz.

| CSV value | Effect |
|---|---|
| *(blank)* | no change |
| `0`–`5` | `rating = <n>`, `rated_at = now` |
| `?` | `rating = NULL`, `needs_relisten = 1` |
| `c` | `is_classical = 1`, rating untouched |

Matching is by exact `file` path against `albums.path`, after stripping any
trailing slash. **It must run after a `scan`**, since it only updates rows that
already exist.

The measurement in §2.6 is what makes this safe to do without ceremony: all
1,848 CSV paths already exist in MPD, so there is no unmatched-path case to
design around. The single duplicate (`Compilations/Sound Environment`) is
ignored — last row wins, to be corrected by hand afterwards if it matters.

Expected result, verifiable with `album-tool status` or plain SQL:
**416 ratings, 69 `is_classical`, 17 `needs_relisten`.**

Genuinely throwaway — once the ratings are in the database, the database is the
source of truth and the CSV is history.

---

## 7. `album-tool sync`

```
album-tool sync [--limit N] [--force]
```

Fetches MusicBrainz release-group data for albums where
`mb_sync_state = 'pending'`, `status = 'present'`, `match_policy != 'never'`,
and `mb_release_group_id IS NOT NULL`.

`--force` re-syncs rows already `ok`. `--limit` caps requests for a partial
run.

### 7.1 Request discipline

MusicBrainz policy compliance is non-negotiable:

- **Maximum 1 request/second**, enforced by the client, not by hope.
  `musicbrainz_rs` ships its own leaky-bucket limiter (5-burst, 1/s refill);
  whether that alone satisfies this or needs wrapping with the injected
  `Clock` for deterministic tests is a `sync`-implementation decision, not a
  design one (see `AGENTS.md`).
- **A descriptive User-Agent identifying the application and a contact
  address.** Anonymous or generic agents get blocked. Set via
  `MusicBrainzClient::new(user_agent)`, not a global function.
- No authentication is needed.

Endpoint: `/ws/2/release-group/<mbid>?inc=genres&fmt=json`

`inc=genres`, **not** `inc=tags` (§2.2).

**Deduplicate by `mb_release_group_id` before requesting** — 1,830 distinct
groups across 1,886 albums, and multiple albums can share one group. Fetch each
group once, apply to every album referencing it.

### 7.2 Failure handling

Transient failures are real — one `503` in four sample requests (§2.3).

- Retry on `503` and `5xx` with exponential backoff, at least 3 attempts.
- On `404`, set `mb_sync_state = 'failed'` with the reason in `mb_sync_error`;
  the MBID is stale and needs fixing in Picard.
- On give-up, set `failed` and record the error. Never leave a row `pending`
  after a genuine attempt, or a re-run will retry it forever.
- **The sync is resumable**: state is committed per album, so an interrupted
  run resumes exactly where it stopped. A full run is ~31 minutes, so it does
  not need to survive across days.

### 7.3 What is written

- `album_genres` rows with `source = 'mb'`, one per genre, carrying its vote
  count. Existing `source = 'mb'` rows for that album are replaced;
  `source = 'tag'` rows are left alone.
- `primary_genre` — the highest-voted MB genre, cached for cheap queries.
- `mb_original_count` — the track count of the earliest release in the release
  group, which gives a real original-track count. The *tagged* release includes
  bonus tracks and cannot supply this. Requires
  `inc=releases` on the release-group request. Where this is confidently
  derivable and `original_tracks` is unset, `album-tool` may set
  `original_tracks = '1-<n>'` with `original_tracks_src = 'mb'`; a `manual`
  value is never overwritten.
- `mb_sync_state = 'ok'`, `mb_synced_at = now`.

Boolean category flags are **not** written by `sync` in v1. Deriving
`is_classical` / `is_spoken` / `is_soundtrack` from MB genres is left until the
genre data exists and can be inspected — the 69 CSV `c` marks already give a
better classical seed than any rule (§2.6).

---

## 8. `album-tool status`

```
album-tool status
```

Read-only report:

- Album counts by `tag_state` and by `mb_sync_state`.
- Albums with `status = 'missing'` — directories that vanished and could not be
  matched to a rename.
- `mixed_mbid` and `partial_mbid` albums, **for manual fixing in Picard**. Per
  the brief, the tool reports and never edits files. Currently:
  `Billy Bragg/The Million Things That Never Happened` and
  `Dire Straits/Brothers in Arms`.
- Albums with `mb_sync_state = 'failed'` and their errors.
- Rating coverage, and albums with `needs_relisten = 1`.

---

## 9. Deferred

Explicitly out of scope for v1, with the schema already in place so none needs
a schema change.

- **`album-tool verify`** — re-check tags against current MusicBrainz data and
  flag drift, merged releases, and disagreements, writing `mb_verify_state`,
  `mb_verified_at`, `mb_verify_notes`. The columns exist from day one.
- **`album-tool export`** — CSV out, for spreadsheet round-tripping.
- **All controller/menu-bar features.** Candidates, in rough order of value:
  rating the current album from the menu; showing rating and original year in
  album menu labels; "play a random 5-star album, excluding classical and
  spoken word"; playing only the original tracks via `original_tracks`.
- **Category derivation from MusicBrainz genres** (§7.3).

---

## 10. Implementation order

Each step is independently verifiable. Do not start a step before the previous
one is confirmed working against the real library.

1. **Workspace split.** Move `src/` → `crates/controller/src/`, create the
   workspace root, add empty `albumdb` and `album-tool` crates. **Build and run
   the menu-bar app; confirm the tray icon, menu and `mpdcontrol://` URL scheme
   all still work.** Nothing else changes.
2. **`albumdb`**: schema, pragmas, domain types, `original_tracks`
   range parser. Unit tests for the parser and for `tag_state` derivation as a
   pure function over track tag sets.
3. **`album-tool scan`**: MPD reading with correct chunking and quoting, album
   grouping, `tag_state`, title/artist resolution. Verify against the counts in
   §2.1 and §5.4 — those tables are the acceptance criteria.
4. **Rename reconciliation**: fingerprinting and the missing/rename logic.
   Test by renaming a directory and re-scanning; the rating must survive.
5. **Ratings import script** (Python, one-off). Verify 416 ratings, 69
   `is_classical`, 17 `needs_relisten` by inspecting the data.
6. **`album-tool status`**.
7. **`album-tool sync`**: rate limiting, User-Agent, retry/backoff,
   deduplication by release group, resumability. Test with `--limit 5` before
   any full run.

### Acceptance

A full `scan`, then the ratings import script, then `sync`, against the live
library produces a database where:

- 1,886 albums are present with the `tag_state` distribution in §5.4.
- 416 ratings, 69 `is_classical`, 17 `needs_relisten` are set.
- ~1,830 release groups have MB genres, with several genres each rather than one.
- A second `scan` changes nothing, and no user-editable field is modified by any
  automated command.
