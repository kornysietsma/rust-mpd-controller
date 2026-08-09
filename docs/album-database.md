# The album database

`albums.db` — a SQLite album database derived from the MPD library and
MusicBrainz, maintained by the `album-tool` CLI (`crates/album-tool`) through
the `albumdb` library crate (`crates/albumdb`). Consumed by hand in SQL, by
agent skills, and eventually by the menu-bar app.

Notes here cover what isn't obvious from the code: the constraints, the
invariants, and the measured facts that shouldn't be re-derived by guesswork.
Crate-API sharp edges (`mpd_client`, `musicbrainz_rs`, `rusqlite`) live in
[`AGENTS.md`](../AGENTS.md); deferred features are in
[future_expansions.md](future_expansions.md).

## Where the database lives, and why it isn't configurable

`albums.db` sits **inside the music folder**, so it's backed up and moved with
the music it describes.

The path is not hand-configured and `album-tool` has **no config file of its
own**. It reads MPD's `~/.mpd/mpd.conf` off disk (via the `mpd-conf` crate) for
`music_directory` and `port`, then uses `<music_directory>/albums.db`. MPD's
*protocol* `config` command is rejected for non-local clients, but reading the
file directly sidesteps that — `album-tool` always runs on the same machine as
MPD, which it must anyway to reach the database file. `mpd_host` is hardcoded
to `localhost` for the same reason (`bind_to_address` doesn't reliably name a
reachable address).

`--mpd-conf <path>` and `--db-path <path>` override independently.
**`--db-path` bypasses only the database path**: `scan` still reads `mpd.conf`
for the port, because it still has to talk to MPD. That's deliberate —
`--db-path` exists so a scratch database doesn't have to sit inside a real
`music_directory`, not to let `album-tool` avoid MPD.

The menu-bar app derives the same two values the same way, so the two agree by
construction rather than by both being configured correctly. It has the
equivalent overrides in `~/.config/mpd-controller/config.toml` (`mpd_conf`,
`db_path`) since a GUI app takes no command line — see the README. Like
`album-tool`, an unreadable `mpd.conf` or an unresolvable/missing `albums.db`
is fatal — the difference is only in how each reports it: `album-tool` exits
with an error on the CLI, the menu-bar app has no terminal to print to, so it
puts the same failure up as a `⚠️` tray icon instead
(`crates/controller/src/config.rs`'s `ConfigError`, see the README). It
validates that `albums.db` exists at startup even though nothing reads it yet.

### `journal_mode = DELETE` is load-bearing

```sql
PRAGMA journal_mode = DELETE;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
```

**The reason is backup integrity, not sync.** This is not about anything
writing *into* the database from outside — nothing does. It is about what
happens when a file-copying backup *reads* it.

A WAL database is two files: recent commits live in `albums.db-wal` and only
land in `albums.db` at a checkpoint. Any copier that takes files
independently — Dropbox, an SMB copy to the NAS, a plain `cp` — can capture
`albums.db` and `albums.db-wal` at different moments. Restoring that pair is
safe *only* if no checkpoint happened in between; if one did, the WAL was
reset and its frames assume a main file that has already absorbed the
checkpointed pages, so replay yields a database missing them. The live file
stays fine — it is the **backup** that is quietly corrupt, which you discover
at the worst possible moment. This is why `VACUUM INTO` and the backup API
exist: copying a live WAL database file-by-file is a known footgun.

`DELETE` mode has no permanent sidecar, so every snapshot is self-consistent.

**The cost, measured** (1.9 MB database, this machine, 2026-08-07):

| | `DELETE` | `WAL` |
|---|---|---|
| Single-row commit | 3.46 ms | 0.93 ms |
| Full-table transaction | 64.7 ms | 58.6 ms |
| Indexed row read | 91 µs | 61 µs |

WAL is genuinely faster; the absolute numbers are irrelevant at this scale. The
menu-bar app writes one row per rating click and `album-tool scan` runs by
hand.

The menu-bar app holding a connection open 24/7 makes `DELETE` *more*
attractive than it was when this was chosen: `-wal` and `-shm` exist for as
long as any connection is open, so under WAL they would go from appearing
briefly during an `album-tool` run to being permanent fixtures the backup
picks up on every rating change.

Multi-writer safety is a non-goal: the folder is synced between machines but
never written from two at once.

### If a GUI tool switches the database to WAL

`journal_mode = WAL` is persisted in the **file header**, not per connection,
so a tool like DBeaver can flip it and the setting outlives that tool's
session. Measured behaviour:

- **Anything can flip `DELETE` → `WAL` at any time**, including while the
  menu-bar app holds a connection open. A running app is not protection.
- The reverse, `WAL` → `DELETE`, needs exclusive access. `albumdb`'s
  `apply_pragmas` runs it on every open, so the next `album-tool` run or app
  restart converts it back — checkpointing the WAL, deleting both sidecars,
  preserving all data.
- That reset fails with `SQLITE_BUSY` if another connection holds the database
  at that moment. The error propagates rather than passing silently, but the
  file stays in WAL until a later open succeeds.
- Setting `DELETE` on a database already in `DELETE` needs no lock and never
  fails, even against a connection mid-transaction — so applying the pragma is
  safe in the normal case.

If you edit the database in a GUI tool, close it afterwards, and check for a
stray `albums.db-wal` next to the database. While one exists, backups of it
are not trustworthy.

## The measured baseline

Measured against the live library; recorded so nobody re-derives them by
eye. **Don't "tune" any derivation rule without re-measuring.**

| | |
|---|---|
| Songs / album directories | 29,028 / 1,886 |
| Top-level split | `Albums/` 1,663, `Compilations/` 196, `Unnmatched/` 27 |
| Multi-disc directories | 195 |
| Distinct release groups | ~1,830 across 1,886 albums (several albums share one) |
| `OriginalDate` differs from `Date` | 421 directories |
| Ratings imported | 416 rated, 69 `is_classical`, 17 `needs_relisten` |
| Full `sync` runtime | ~31 minutes at 1 request/second |

`tag_state` distribution, which a correct `scan` reproduces exactly:

| State | Condition | Count |
|---|---|---|
| `clean` | every track has an MBID, all the same | 1,833 |
| `mixed_mbid` | every track has an MBID, >1 distinct value | 2 |
| `partial_mbid` | some tracks have an MBID, some don't | 1 |
| `no_mbid_consistent` | no MBIDs; one `Album`, one `AlbumArtist`, one year | 43 |
| `no_mbid_varied` | no MBIDs; `Album` title or year varies | 3 |
| `no_mbid_incomplete` | no MBIDs; `Album` or `AlbumArtist` absent | 4 |

**50 albums stay `mb_sync_state = 'pending'` forever** — they're exactly the
50 with no MBID at all, so there is nothing for `sync` to look up. That's the
expected steady state, not an unfinished run.

### Why MusicBrainz is needed at all

Tags already carry original release dates, so MusicBrainz is *not* the primary
date source — the files are. What it's for is **genres**:

- Every tagged track carries **exactly one** genre (27,250 with one, 1,778
  with none, zero with more than one). That's Picard's default configuration,
  not an MP3 limitation.
- MusicBrainz returns 7–10 **vote-weighted** genres per release group instead.
- **Genres live on the release group, not the release** — sampled releases
  returned zero release-level genres.
- Use `inc=genres`, **never `inc=tags`** — the tags endpoint mixes in
  folksonomy noise (`english`, `ph_5_stars`, moods, years).

## The one non-negotiable invariant

**Every user-editable column survives every automated pass**: `rating`,
`needs_relisten`, `rated_at`, `is_classical`, `is_spoken`, `is_soundtrack`,
`original_tracks`, `original_tracks_src`, `match_policy`, `notes`.

This is enforced *structurally*, not by care: `upsert_album` omits all of them
from both its `INSERT` column list and its `ON CONFLICT ... SET` clause, so it
is incapable of touching them. `title`/`artist`/`title_source` are the one
pair of columns a scan does write, and they carry an explicit
`CASE WHEN title_source = 'manual'` guard so a hand-entered title survives too.

If you extend `upsert_album`, keep that property. The test
`upsert_never_touches_user_editable_columns` exists to catch a regression, and
deliberately asserts that derived fields *did* change in the same pass, so it
can't pass by doing nothing.

Related deliberate choices:

- **Album-level only — there is no tracks table.** Anything track-shaped is
  queried live from MPD. `track_count`/`disc_count` are cached only because
  `status` needs them.
- **Category is three boolean flags, not an enum.** ~90% of the library would
  land in one "popular" bucket, so the value is *exclusion*
  (`WHERE NOT is_classical AND NOT is_spoken AND rating >= 4`), and flags
  handle genuine overlap — a classical soundtrack is both.
- **`needs_relisten` is orthogonal to `rating`**, so it can also mean "rated
  four years ago, no longer trust it" — which a sentinel rating can't.
- **Clearing a rating leaves `rated_at` alone.** It records when the album was
  last rated, not whether it currently holds a rating.
- **`match_policy = 'never'`** permanently opts an album out of automated
  MusicBrainz matching — the directory wins.

## `scan`

Reads MPD, rewrites derived columns, idempotent, no network beyond MPD.

**MPD surfaces every tag this project needs, so nothing here parses audio
files.** `Album`, `AlbumArtist`, `Artist`, `Title`, `Track`, `Date`,
`OriginalDate`, `Genre`, `Label`, `Disc` and all six MusicBrainz ID tags come
straight off the protocol. **No `id3`/`lofty`-style tag-reading crate is needed
anywhere in this workspace** — if one starts to look necessary, check whether
MPD already exposes the field first.

The library is read in chunks — `listfiles` for top-level directories,
`lsinfo` per top-level directory for chunk boundaries, then
`find "(base '<chunk>')"` per chunk — because `find "(base '')"` over the whole
library returns zero rows (response too large). Each top-level directory is
also queried directly to catch loose files, and results are de-duplicated by
path. Songs are grouped by containing directory; **multi-disc albums are one
directory with continuous track numbering**, not `CD1/`/`CD2/` subdirectories.

### `tag_state`'s two counter-intuitive rules

Recomputed from scratch every scan, which is what makes re-tagging in Picard
work with no extra mechanism: an album that gains MBIDs moves
`no_mbid_* → clean` on the next scan and `sync` picks it up on the one after.

1. **Artist variation is not a signal.** Only `Album`, `AlbumArtist` and year
   are compared. A compilation with one `AlbumArtist` and 19 different track
   artists is `no_mbid_consistent`, not `no_mbid_varied`.
2. **The junk year `'0000'` is filtered out before counting distinct years**,
   and years are the first four characters of `Date`. Mozart's
   *Eine Kleine Nachtmusik* carries `1787` on some tracks and `'0000'` on
   others; that's one year, not two. An earlier measurement got 42
   `no_mbid_consistent` instead of 43 by missing this.

`Date` and `OriginalDate` of `0000` normalise to `NULL` everywhere.

### Title and artist

Tags when trustworthy, directory path otherwise; `title_source` records which.

| `tag_state` | Source | `title_source` |
|---|---|---|
| `clean`, `mixed_mbid`, `partial_mbid`, `no_mbid_consistent` | tags | `tag` |
| `no_mbid_varied`, `no_mbid_incomplete` | path | `path` |

| Path pattern | title | artist |
|---|---|---|
| `Albums/<letter>/<artist>/<album>` | `<album>` | `<artist>` |
| `Compilations/<album>` | `<album>` | `Various Artists` |
| `Unnmatched/<artist>/<album>` | `<album>` | `<artist>` |
| `Unnmatched/<album>` | `<album>` | `NULL` |

A path matching none of the four is a **hard error, not a best-effort guess**
— it means the data is wrong or a new directory shape has appeared, and both
are worth stopping for.

The file-tag genre is written to `album_genres` with `source = 'tag'` and
`votes = 0`.

### Disappearances, and why renames aren't detected

A path in the database but not in MPD becomes `status = 'missing'` and is
**never deleted** — deleting would silently discard a rating. Marking a row
missing does *not* update `last_seen`, which records when the album was last
seen *present*.

There is **no automatic rename detection**. A renamed directory shows up as
one new unrated row plus one `missing` row that keeps its rating, and `scan`
prints both by path so the pairing is obvious. Fixing it by hand means
repointing the old row's `path` and deleting the duplicate.

This was cut deliberately. An earlier design matched disappearances to new
albums by content fingerprint, on the untested assumption that renames would
be common. Since nothing is ever deleted, the real cost of not automating is
"one row to tidy up", not lost data — not worth the machinery. Revisit only if
`status` starts showing renames as a repeated nuisance.

## `sync`

Fetches release-group data for albums that are `present`, `match_policy !=
'never'`, have an `mb_release_group_id`, and are `mb_sync_state = 'pending'`.
`--force` widens that to `ok` and `failed` too; `--limit N` caps requests.

### Request discipline is non-negotiable

- **Max 1 request/second, enforced by the client.** `musicbrainz_rs` 0.13.0's
  own limiter is unreachable through any public feature flag (see AGENTS.md),
  so `album-tool` has its own `RateLimiter` on `std::time::Instant` — a
  monotonic clock, deliberately *not* the injected wall-clock `Clock` used for
  storage timestamps.
- **A descriptive User-Agent with a contact address.** Anonymous or generic
  agents get blocked.
- Endpoint: `/ws/2/release-group/<mbid>?inc=genres+releases+media`.
  **`media` is required** — `releases` alone carries no track-count data, so
  without it `mb_original_count` is always `NULL`.
- **Deduplicate by release group before requesting.** ~1,830 groups cover
  1,886 albums; fetch once, apply to every album referencing it.

### Failure states

Transient failures are real — one `503` in four sample requests.

- Retry `5xx` with exponential backoff, at least 3 attempts.
- A `404` means the MBID is stale and needs fixing in Picard: record
  `failed` with the reason and **never auto-retry it**.
- On give-up, also record `failed`. **Never leave a row `pending` after a
  genuine attempt**, or every re-run retries it forever.
- **Resumable**: state is committed per album as it happens, not in one
  transaction for the run, so a hard interrupt loses nothing already done.

### What a success writes

One transaction, so a row is never observably `ok` without the data
justifying it:

- `album_genres` rows with `source = 'mb'` and vote counts, **replacing** any
  previous `mb` rows. `source = 'tag'` rows are left alone, and vice versa.
- `primary_genre` — the highest-voted MB genre, cached for cheap queries.
- `mb_original_count` — track count of the earliest release in the group, which
  is what gives a real original-track count; the *tagged* release usually
  carries bonus tracks and can't. Earliest is picked by comparing raw date
  strings lexicographically (exact for equal precision, approximate across
  mixed precision — fine for a cache column); releases missing a date or media
  are excluded.
- `original_tracks = '1-<n>'` / `original_tracks_src = 'mb'`, but **only when
  `original_tracks` is still `NULL`**. An existing value, manual or from a
  prior sync, is never overwritten.

`sync` does **not** set the category flags. Deriving `is_classical` /
`is_spoken` / `is_soundtrack` from MB genres waits until the genre data can be
inspected properly — the 69 hand-marked classical albums are already a better
seed than any rule.

## `status`

Read-only, and the only command that reports rather than changes anything.
Album counts by `tag_state` and `mb_sync_state`; `missing` directories;
`mixed_mbid`/`partial_mbid` albums **for fixing by hand in Picard** — the tool
reports tag problems and never edits audio files; `failed` syncs with their
recorded errors; rating coverage and `needs_relisten`.

## Error handling

`anyhow::Result` at the CLI boundary, and one rule that shapes the rest:
**expected conditions are values, not errors.** A MusicBrainz 404 sets
`mb_sync_state = 'failed'` with a reason and the run carries on — it is a
fact about that album, not a failure of the command. An unreadable database
or an unparseable `mpd.conf` is a genuine error and aborts.

The notable case on the aborting side is an album path matching none of the
four known patterns: that means the data is wrong or a new directory shape has
appeared, and silently inventing a title would be worse than stopping. The
handful of `assert!`/`expect!` calls guard internal invariants (an album group
is never empty; `pending_sync_albums` only returns rows with an MBID), not
input.

## Schema changes are manual

All statements use `IF NOT EXISTS`. There is deliberately **no migration
mechanism and no version marker** — for a single-user tool, a hand-applied
change would leave a marker stale, and a stale marker is worse than none.

To change the schema: back up the file (or `.dump` it to SQL), then
`ALTER TABLE ADD COLUMN`, or the create/copy/drop/rename dance for things
SQLite can't alter in place (such as changing a `CHECK` constraint). Both
preserve existing data.

The `mb_verify_*` columns are already in the schema for the deferred
`album-tool verify` command, so adding it later needs no schema change.

## The testability boundary

The structural decision everything else follows from. `mpd_client`'s
`Song`/`Status` are `#[non_exhaustive]` with no public constructor, and
`musicbrainz_rs::Release` has no `Default` — none of them can be built in a
test.

```
  MPD  ──parse──▶  Vec<SongTags>  ──pure──▶  AlbumFacts  ──▶  albumdb
       (I/O, thin)   plain struct    (all the logic)          (sqlite)
```

`SongTags` is a plain owned struct with named fields, not a string-keyed map —
the set of tags needed is fixed and known, so compiler-checked field names win.
Everything interesting (grouping, `tag_state`, title/artist resolution,
earliest-release picking) is a pure function over plain domain structs and is
tested with hand-built values. The I/O layers exist only to produce those
structs, and each sits behind a trait — `LibrarySource`, `MusicBrainzClient`,
`Clock` — so a fixture or fake substitutes for it.

`ReleaseGroupData`/`MusicBrainzError` are `album-tool`'s own types rather than
`musicbrainz_rs`'s, for the same reason `SongTags` isn't `mpd_client`'s: it
keeps the dependency out of the trait's shape.

Tests run against a committed **synthetic** fixture
(`crates/album-tool/tests/fixtures/library.txt`) — invented album and artist
names, because the real library is gitignored and this repo is public. Each
awkward case in it is present deliberately, not by luck.
