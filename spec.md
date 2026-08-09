# Spec: album data in the menu-bar app

Show the current album's database record in the `MpdController` tray menu, and
allow editing its rating and category flags from there.

Derived from `album-ui-goals.md`. Every design choice below was made
deliberately; where a cheaper or more obvious alternative was rejected, the
reason is recorded so it isn't silently re-litigated.

---

## 1. Scope

**In scope**

- Read the current album's row from `albums.db` and display: original year,
  rating, `needs_relisten`, `is_classical`, `is_spoken`, `is_soundtrack`.
- Set the rating to 0–5 from a submenu. **Clearing a rating back to unrated is
  deliberately not offered** — see §4.3.
- Toggle each of the four flags from the same submenu.
- Detect changes to `albums.db` made by anything else (`album-tool`, a GUI
  tool such as DBeaver, hand SQL, agent skills) and refresh the menu.

**Out of scope**

- Any effect on playback, queue order, or the prev/next-album navigation. The
  album section is inert with respect to MPD.
- `mpdcontrol://` verbs for rating or flagging — deferred, see §12.
- Editing any other column (`notes`, `original_tracks`, `match_policy`,
  `title`, …).
- Multi-writer safety. Already an explicit non-goal of the database
  (`docs/album-database.md`); nothing here changes that.

---

## 2. Verified facts

Measured against the live MPD instance and `album-data/current-albums.db` on
2026-08-07. Recorded so the implementer doesn't re-derive them by eye.
**Re-measure before changing any rule that depends on these.**

### Path mapping is exact and total

MPD song URLs are library-relative, e.g.

```
Albums/T/The Decemberists/I'll Be Your Girl/07 Everything Is Awful.mp3
```

and the `albums.path` column holds exactly the parent directory of that URL:

```
Albums/T/The Decemberists/I'll Be Your Girl
```

Comparing the set of distinct song-parent-directories in MPD against the set of
`albums.path` values:

| | |
|---|---|
| Distinct album directories in MPD | 1886 |
| Rows in `albums` | 1886 |
| In MPD but not the database | 0 |
| In the database but not MPD | 0 |

`menu.rs::album_key(url)` already computes precisely this string. So the
lookup is `albumdb::find_by_path(conn, album_key(url))` with **no
normalisation, no fallback, and no fuzzy matching**. No album in this library
nests discs in subdirectories — `docs/album-database.md` already records that
multi-disc albums live in one directory with continuous track numbering.

### Column coverage

| Column | Populated | Notes |
|---|---|---|
| `original_date` | 1813 / 1886 | formats: `YYYY` (311), `YYYY-MM` (149), `YYYY-MM-DD` (1353) |
| `date` | 1870 / 1886 | same three formats |
| neither date | 16 | |
| `date` only, no `original_date` | 57 | |
| `original_date` only, no `date` | 0 | |
| years differ between the two | 365 (19%) | e.g. `Alice Cooper/Welcome to My Nightmare`: `date=1987`, `original_date=1975-03-11` |

### Rating and flag usage

| Value | Count |
|---|---|
| `rating IS NULL` | 1470 |
| `rating = 0` | 1 |
| `rating = 1`, `2`, `3` | 0, 0, 0 |
| `rating = 4` | 187 |
| `rating = 5` | 228 |
| `needs_relisten = 1` | 17 (**all of them unrated**) |
| `is_classical = 1` | 69 |
| `is_spoken = 1` | 0 |
| `is_soundtrack = 1` | 0 |
| `rating IS NULL AND rated_at IS NOT NULL` | 0 |

The scale is in practice being used as *unrated / good / great*, with
`needs_relisten` as a to-do marker on unrated albums. The full 0–5 range is
still offered (§4.3) — `0` is semantically distinct from `NULL`
("listened, it's bad" vs "not yet judged").

### Change-signal behaviour

Measured on a copy of the real database, comparing the two candidate poll
signals. Both fire exactly on another process's commit and neither
false-positives on reads or empty commits — so the choice (§6.3) turns on the
last two rows, not on cost.

| Event | `stat()` | `PRAGMA data_version` |
|---|---|---|
| Our own write | fires (noise) | silent — correct, we handle it directly |
| Another process commits | fires | fires |
| Another process only reads | silent | silent |
| Zero-row `UPDATE` (empty commit) | silent | silent |
| Mid-transaction, uncommitted | silent | silent |
| File replaced (new inode) | fires | **blind** |
| Commit while the file is in WAL mode | **blind** | fires |
| Cost per tick | 12 µs | 54 µs |

Reading `data_version` also refreshes the connection's view of the data — the
new value is visible to the very next `SELECT`.

The mid-transaction row is worth a note: it holds because the whole database
is 467 pages against SQLite's default 2 MB page cache, so nothing reaches the
main file before commit. That margin is thin — past roughly 2 MB a large
`album-tool scan` transaction could start spilling. It does not affect the
chosen design, which never looks at file metadata.

### There is no menu-open hook

Checked in `tray-icon` 0.24.2, `src/platform_impl/macos/mod.rs`:
`mouseDown:` calls `send_mouse_event(…, Click, Down)` and then immediately
`on_tray_click`, which calls `performClick`. `performClick` opens the menu in
a **modal tracking loop**. Our click event is delivered through
`EventLoopProxy::send_event`, so the `tao` event loop cannot process it until
after the menu closes.

Consequences, both load-bearing for this design:

- **We cannot refresh the menu at the moment it opens.** Freshness must come
  from something that runs ahead of the click — hence the poll in §5.
- **The menu cannot change while it is displayed.** Any events arriving during
  the modal loop are queued and applied after it closes. This is why there is
  no optimistic-UI problem: clicking an item closes the menu, and the rebuild
  happens while it is shut.

`muda` exposes no menu-delegate callback either — `tray-icon` already installs
the status item as the `NSMenu` delegate and does not forward
`menuNeedsUpdate:`.

---

## 3. What the user sees

The album section lives inside the existing track section, after the album
copy row and before "In playlists".

```
⏯ Play/Pause
⏭ Next
⏹ Clear
──────────────
⏮⏮ Prev Album: Beautiful Freak
⏭⏭ Next Album: Daisies of the Galaxy
──────────────
⧉ Novocaine for the Soul - EELS
⧉ EELS TIME!
1996 · ★★★★☆ · ↻ · Soundtrack        ← disabled, info only
Album ▸ ────────────────────┐
        │ Rating ▸          │
        │ ───────────────── │
        │ ✓ Relisten        │
        │   Classical       │
        │   Spoken          │
        │ ✓ Soundtrack      │
        └───────────────────┘
In playlists ▸
Add to ▸
──────────────
Quit
```

with `Rating ▸` opening:

```
  ☆☆☆☆☆  0
  ★☆☆☆☆  1
  ★★☆☆☆  2
  ★★★☆☆  3
✓ ★★★★☆  4
  ★★★★★  5
```

On an album with no rating yet — 1470 of 1886 today — **no item is checked**.
That is correct and expected; do not "fix" it by defaulting the tick to `0`.

Rejected alternatives: everything inside one submenu (the data is worth seeing
without a hover); fully flat inline (adds ~6 rows to an already busy menu).

---

## 4. Display rules

### 4.1 The summary row

A **disabled** `MenuItem`, matching the convention already used for the
"In playlists" rows and the startup-error rows.

Built by joining present parts with `" · "` (U+00B7 MIDDLE DOT, spaced), in
this fixed order:

1. **Year** — omitted when unknown. See §4.2.
2. **Rating** — five star glyphs, omitted entirely when `rating IS NULL`.
   `★` = U+2605, `☆` = U+2606. `rating = 0` renders as `☆☆☆☆☆`, which is
   visually distinct from the row having no rating part at all.
3. **Relisten** — `↻` (U+21BB) when `needs_relisten = 1`, otherwise omitted.
4. **Flags** — the words `Classical`, `Spoken`, `Soundtrack`, in that order,
   each present only when its column is `1`.

**If every part is absent, the row is not emitted at all.** An unrated,
undated, unflagged album gets no summary row — which is 1470-ish albums today,
so this matters.

Worked examples:

| State | Row |
|---|---|
| year, rating, relisten, soundtrack | `1996 · ★★★★☆ · ↻ · Soundtrack` |
| year + rating only | `1996 · ★★★☆☆` |
| year, rating, two flags | `2003 · ★★☆☆☆ · Classical · Spoken` |
| unrated, no year, flagged | `↻ · Classical` |
| unrated, no year, no flags | *(row omitted)* |
| readable database, no row for this path | `not in album database` |
| database unreadable | `album database unavailable` |
| last write failed | `album database write failed` |

**Glyph presentation.** `menu.rs` pins several glyphs to text presentation with
U+FE0E because they otherwise fall back to wide colour emoji. U+2605, U+2606,
U+21BB and U+00B7 have **no emoji presentation form**, so they need no
variation selector. Do not add one — it would render as a stray box on some
fonts.

### 4.2 Which year

`original_date`'s year, falling back to `date`'s year, omitted when neither is
present.

Extraction: take the first four characters of the string. Given the three
formats measured in §2 (`YYYY`, `YYYY-MM`, `YYYY-MM-DD`) that is always the
year. **Guard it anyway**: only accept the slice if all four characters are
ASCII digits; otherwise treat the column as absent. Cheap, and it means a
malformed row degrades to "no year" rather than printing garbage into the menu
bar.

This is a pure function and one of the primary unit-test targets (§10):

```rust
fn year_of(original_date: Option<&str>, date: Option<&str>) -> Option<&str>
```

Rejected: showing the edition year alongside (`1975 (ed. 1987)`) — accurate,
but costs width on one album in five for information that isn't what the goals
doc asked for. Rejected: never falling back, which would leave 57 albums
showing no year despite a perfectly good one being available.

### 4.3 The `Album ▸` submenu

Emitted **only when a database row was found**. Hidden when nothing is
playing, when the current file sits at the library root (no parent directory,
same rule that already hides the album copy row), when there is no row for the
path, and when the database is unavailable — in each of those cases there is
nothing to edit.

Contents, in order:

1. `Rating ▸` submenu — six `CheckMenuItem`s, `0`–`5`, each prefixed with its
   star rendering. At most one is checked, reflecting the stored value. Nothing
   is checked when `rating IS NULL`, or in the unreachable case of a stored
   value outside 0–5 (the schema `CHECK` prevents it, but the render must not
   panic).
2. A separator.
3. Four `CheckMenuItem`s: `Relisten`, `Classical`, `Spoken`, `Soundtrack`,
   each checked from its column.

**There is no `Unrated` entry, by decision.** Unrated is a state the UI
displays but cannot produce: it means "not yet judged", and once judged there
is no going back — `Relisten` is the marker for "rated, but I'm not sure".
Since the menu can never write `NULL`, `AlbumRequest::SetRating` carries a
plain `u8` rather than an `Option<u8>` (§7.2), making the unreachable case
structurally impossible rather than merely unused.

`albumdb::set_rating` keeps its `Option<u8>` parameter — it is the general
library API and `album-tool` may still want to clear a rating — but the
controller never passes `None`.

`muda` toggles a `CheckMenuItem`'s own checked state on click, but that state
is discarded — the menu is rebuilt from a fresh read after every write, so the
displayed ticks always reflect what is actually stored.

---

## 5. Write semantics

### 5.1 Rating

The only rating write the menu can produce, for a chosen value `0`–`5`:

```sql
UPDATE albums
   SET rating = ?, rated_at = ?, needs_relisten = 0
 WHERE album_id = ?
```

One statement, so the two writes cannot land separately.

There is no clear-the-rating path from the menu (§4.3). `albumdb::set_rating`
still implements one for `None` — leaving `rated_at` in place as a record of
when it was last rated, and not touching `needs_relisten` — but nothing in the
controller reaches it.

**Setting a rating clears `needs_relisten`.** This matches the usual workflow
("if I rate something I've made a decision") and matches the data — all 17
relisten albums are unrated.

**Accepted, documented tradeoff:** ticking `Relisten` and *then* setting a
rating silently discards the tick. Getting both requires setting the rating
first and ticking `Relisten` second. Because every `muda` click closes the
menu, either ordering costs two menu-opens anyway; this choice puts the extra
open on the rarer path. Note this in `docs/usage.md` so it reads as intended
behaviour rather than a bug.

### 5.2 Flags

Each of the four is a plain `UPDATE` of its own column. The click carries the
**target** value, not "toggle" — the menu id encodes it (§7.3). This is
deliberate: if the row changed underneath between the menu being built and the
item being clicked, you get the value you actually clicked for, not an
inversion of some state you never saw.

### 5.3 Every write is followed by a re-read

After any write the album-db thread re-reads the row and emits a fresh state
event. The menu is then rebuilt from stored truth. This is what makes a failed
write self-evident: the tick you clicked simply isn't there.

---

## 6. Architecture

### 6.1 A dedicated album-db thread

The main thread runs the `tao` event loop and must never touch SQLite.
`albumdb` sets `busy_timeout = 5000`, so a write colliding with an
`album-tool scan` transaction can block for up to five seconds — on the main
thread that is a frozen menu bar.

Instead, mirror the shape `mpd.rs` already uses: a background worker owns the
resource and pushes results to the event loop through an `EventLoopProxy`.

```
main thread (tao loop)                album-db thread
  MpdEvent ──── SetCurrentAlbum ────▶ remember path, read row
  menu click ── SetRating/SetFlag ──▶ write, then re-read
  rebuild menu ◀──── UserEvent ────── send AlbumState
                                          │
                                      recv_timeout(≤3s)
                                      PRAGMA data_version
                                      changed? re-read
  rebuild menu ◀──── UserEvent ─────────┘
```

Consequences:

- `ControlFlow::Wait` is **unchanged** — no `WaitUntil`, no timer branch in the
  event loop. The poll's timing lives entirely inside the worker.
- The 5s `busy_timeout` can never freeze the UI.
- One new module, one new `UserEvent` variant.

Rejected: doing it inline on the main thread (freezes the menu bar under
contention, and forces `ControlFlow::WaitUntil`); `tokio::spawn_blocking` with
`Arc<Mutex<Connection>>` (`rusqlite::Connection` is `Send` but not `Sync`, so
the mutex is mandatory rather than chosen, and it makes the serialisation
implicit where a channel makes it explicit).

### 6.2 The worker loop

```rust
let mut next_poll = Instant::now() + POLL_INTERVAL;
loop {
    let timeout = next_poll.saturating_duration_since(Instant::now());
    match rx.recv_timeout(timeout) {
        Ok(request)                       => handle(request),
        Err(RecvTimeoutError::Timeout)    => { poll(); next_poll = Instant::now() + POLL_INTERVAL; }
        Err(RecvTimeoutError::Disconnected) => break,   // main thread is gone
    }
}
```

Computing the timeout from a `next_poll` deadline rather than passing
`POLL_INTERVAL` directly means a stream of requests cannot starve the poll.

`POLL_INTERVAL` is **3 seconds**.

### 6.3 Change detection

On each tick, read `PRAGMA data_version` and compare it to the previous value.

| Observation | Action |
|---|---|
| value unchanged | nothing |
| value changed | re-read the row; emit if the state differs |
| the pragma errors | drop the connection, emit `Unavailable`, retry next tick |

Emit only when the resulting state differs from the last one sent, so a write
elsewhere in the database doesn't churn the menu.

On reopening the connection, re-baseline `data_version` — it is a per-connection
value, not a persistent one, so a fresh connection starts its own sequence.

**Why `data_version` and not `stat()`.** Both were measured against the real
database (§2). They are equally precise for ordinary edits, and the cost
difference (12 µs vs 54 µs, once per 3 s) decides nothing. Two things decide it:

- `data_version` is the exact signal — *"another connection committed"* —
  rather than a proxy inferred from file metadata. It is silent for our own
  writes, which we already handle directly.
- **`stat()` goes blind if the database is ever switched to WAL.** Commits
  then land in `albums.db-wal` and the main file's mtime and size never move.
  `journal_mode` is persisted in the file header, so a GUI tool like DBeaver
  can leave it that way — see `docs/album-database.md`. This is a live risk,
  because DBeaver is a real editing path for this database.

What this gives up: `data_version` cannot see the file being *replaced*
(unlink + rename gives a new inode while our connection reads the old one).
That was the original reason for choosing `stat()`, on the assumption Dropbox
might sync a copy inbound. It doesn't — Dropbox is backup-only and strictly
outbound. The remaining path to a replaced file is restoring from backup after
a disaster, at which point restarting the app is a triviality.

**Accepted limitation, document it in the README:** after restoring
`albums.db` from a backup, restart `MpdController`.

The connection is long-lived but holds **no transaction between operations**,
so it does not block `album-tool`'s writes.

### 6.4 Opening the database

The app must **never create or migrate** `albums.db`. `albumdb::open` runs
`CREATE TABLE IF NOT EXISTS` and will happily create an empty database at a
wrong path — silently replacing "my ratings vanished" with "the app works, the
data is gone".

Use `Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)` —
without `SQLITE_OPEN_CREATE`, SQLite errors if the file is absent. That is
race-free, unlike an `exists()` check followed by an open.

`config::load()` already validates that the file is present at startup and
turns its absence into a fatal `ConfigError::DbFileMissing` (⚠ tray icon).
That behaviour is unchanged.

---

## 7. Code changes

### 7.1 `crates/albumdb`

**`connection.rs` — new `open_existing`**

```rust
/// Open an existing database read-write, with pragmas applied and **without**
/// running the schema. For consumers that must never create or migrate the
/// file — the menu-bar app, which would otherwise silently create an empty
/// database at a mistyped path. `album-tool`, which owns the schema, keeps
/// using `open`.
pub fn open_existing(path: &Path) -> Result<Connection>
```

It **does** apply the pragmas, sharing `apply_pragmas` with `open`. That is
deliberate: `journal_mode = DELETE` is how a database some GUI tool left in WAL
gets converted back (see `docs/album-database.md`). Measured behaviour of that
reset:

- On an already-`DELETE` database it is a no-op needing no lock, and succeeds
  even against a connection mid-transaction — so it adds no startup fragility.
- On a WAL database with no other connection open, it checkpoints, deletes both
  sidecars, and preserves all data.
- On a WAL database with another connection open it fails `SQLITE_BUSY`.
  `rusqlite::pragma_update` uses `execute_batch`, so the error propagates
  rather than passing silently — the open fails, the worker reports
  `Unavailable`, and a later retry succeeds once the other tool exits.

**Upgrade `apply_pragmas` to use `pragma_update_and_check` for
`journal_mode`**, asserting the returned mode is `delete` **or `memory`**.
`pragma_update` discards the value the pragma returns, so today the code
cannot tell a successful set from a set that silently reported some other
mode. Errors do propagate, so this is belt-and-braces — but it is cheap, and
this pragma is load-bearing for backup integrity.

The `memory` exception is load-bearing, not decorative: SQLite reports
`journal_mode = memory` for a `:memory:` database regardless of what's
requested — journaling in memory can't be anything else — and
`open_in_memory` (used throughout both crates' test suites) shares
`apply_pragmas`. A `delete`-only assertion breaks `open_in_memory` and every
test built on it. `pragmas_read_back_correctly` already asserted this
behaviour before it became load-bearing here.

**`enums.rs` — new `AlbumFlag`**

Not a schema string enum like the others; this one names columns.

```rust
pub enum AlbumFlag { Relisten, Classical, Spoken, Soundtrack }

impl AlbumFlag {
    pub fn column(self) -> &'static str { … }   // needs_relisten | is_classical | …
}
```

**`queries.rs` — new `set_flag`, replacing `set_needs_relisten`**

```rust
pub fn set_flag(conn: &Connection, album_id: i64, flag: AlbumFlag, value: bool) -> Result<()>
```

The column name is formatted into the SQL string, because SQLite cannot bind
an identifier. **This is safe and must be commented as such**: the name comes
from a closed enum's `column()`, never from caller data. A reviewer (human or
agent) will otherwise flag it as injection.

`set_needs_relisten` is removed — its only caller is one `album-tool` test
(`status.rs`), which switches to `set_flag(.., AlbumFlag::Relisten, ..)`.
Leaving two ways to write the same column invites drift.

**`queries.rs` — `set_rating` also clears `needs_relisten`**

Per §5.1, in a single statement. Update the doc comment.

*Verified:* `album-tool`'s `status.rs` test
`report_covers_tag_states_sync_states_missing_ratings_and_relisten` rates the
album `Albums/A/Artist/Clean` and flags relisten on a *different* album
`Albums/C/Artist/Relisten`, so it is unaffected by the new clear. Confirm the
suite still passes rather than assuming.

**`lib.rs`** — export `open_existing`, `AlbumFlag`, `set_flag`; drop
`set_needs_relisten`.

### 7.2 `crates/controller/src/album.rs` (new)

Owns the thread and everything that touches SQLite. No formatting logic — that
belongs in `menu.rs` with the rest of the pure model layer. The one exception
is `AlbumInfo::year`: it's built by calling `menu::year_of` (§4.2) on the
row's `original_date`/`date`, since year extraction is display logic even
though it has to run before `AlbumInfo` exists. This is the only place
`album.rs` reaches into `menu.rs`.

```rust
/// What the main thread asks of the worker.
pub enum AlbumRequest {
    /// The playing album changed (or there is none).
    SetCurrentAlbum(Option<String>),
    /// `u8`, not `Option<u8>` — the menu cannot clear a rating (§4.3).
    SetRating { path: String, rating: u8 },
    SetFlag   { path: String, flag: AlbumFlag, value: bool },
}

/// The album facts the menu needs, extracted from a row.
pub struct AlbumInfo {
    pub year: Option<String>,
    /// `Option`, unlike `SetRating`'s `u8`: unrated is a state we display
    /// (1470 albums today) even though the menu cannot set it.
    pub rating: Option<u8>,
    pub relisten: bool,
    pub classical: bool,
    pub spoken: bool,
    pub soundtrack: bool,
}

pub enum AlbumState {
    /// Nothing playing, or a library-root file with no parent directory.
    NoAlbum,
    /// The database could not be read.
    Unavailable,
    /// Readable, but no row for this path.
    NotFound,
    Found(AlbumInfo),
}

/// One message to the event loop: the current state, plus any error from the
/// most recent write. `write_error` is cleared by the next successful write.
pub struct AlbumEvent {
    pub state: AlbumState,
    pub write_error: Option<String>,
}

pub fn run(db_path: PathBuf, rx: Receiver<AlbumRequest>, on_event: impl Fn(AlbumEvent))
```

Write requests carry their own `path` rather than relying on the worker's
remembered current album. The request is then self-contained and there is no
cross-thread ordering to reason about — the same reason `main.rs` already
holds `current_url` for the `addto:` path.

On receipt, `SetCurrentAlbum` does two things, matching the architecture
diagram in §6.1 ("`SetCurrentAlbum` ─▶ remember path, read row"): it updates
the path the poll watches, **and** immediately reads and emits that path's
state, rather than waiting for the next poll tick. A freshly-played album
shouldn't take up to 3s to show its data. `SetRating`/`SetFlag` emit
unconditionally too, for the same reason — a menu click is a direct request
and gets an immediate, unconditional response, not just a "poll rules apply"
dedup against the last state sent. Deduping against the last-sent state
(§6.3) applies only to the timer-driven poll, never to a direct request.

### 7.3 `crates/controller/src/menu.rs`

New pure model types, following the existing `MenuModel` pattern:

```rust
pub struct AlbumSection {
    /// The summary row text; `None` means emit no row.
    pub summary: Option<String>,
    /// A disabled notice row (unavailable / write failed).
    pub notice: Option<String>,
    /// `None` means emit no `Album ▸` submenu.
    pub controls: Option<AlbumControls>,
}

pub struct AlbumControls {
    pub rating: Option<u8>,
    pub relisten: bool,
    pub classical: bool,
    pub spoken: bool,
    pub soundtrack: bool,
}
```

`MenuModel` gains `pub album: AlbumSection`, and `show_track_section()` gains
the album section to its `||` chain, so an album with data still shows a track
section even if nothing else in it applies.

New pure functions, all unit-testable:

- `year_of(original_date, date) -> Option<&str>` (§4.2)
- `stars(rating: u8) -> String`
- `album_summary(info: &AlbumInfo) -> Option<String>` (§4.1)
- `album_section(state: &AlbumState, write_error: Option<&str>) -> AlbumSection`

New menu-id families, alongside the existing `album:` / `addto:` / `copy:`
prefixes and their `strip_*` parsers — both halves stay in `menu.rs` so
`main.rs` never knows the wire format:

| Prefix | Payload | Example |
|---|---|---|
| `rating:` | `0`–`5` | `rating:4` |
| `flag:` | `<name>:<0\|1>` | `flag:classical:1` |

`strip_rating_id` and `strip_flag_id` return the unparsed remainder, so
`main.rs` can log a malformed id distinctly from an id belonging to another
item — matching how `strip_album_id` is already used.

`render` gains the album rows. Per the module's own warning, **no decisions go
in `render`** — it only walks `AlbumSection`.

⚠ **Watch the id-prefix collision.** The existing album-*navigation* prefix is
`album:` and the new flag prefix is `flag:`. `strip_album_id` uses
`strip_prefix`, so `flag:` is unambiguous — but do not name the new prefix
anything beginning with `album`.

### 7.4 `crates/controller/src/main.rs`

- New `UserEvent::Album(AlbumEvent)` variant.
- Spawn the album worker (a plain `std::thread`, not a tokio task — it is
  blocking, long-lived, and owns a `!Sync` resource) with an
  `EventLoopProxy` clone, exactly as `mpd::run` is wired.
- Hold `current_album_path: Option<String>` alongside the existing
  `current_url`, so a `rating:` / `flag:` click can name the album the menu
  was built from.
- Hold the last `MenuModel` across events. **This is the one real refactor
  here.** The menu is now built from two independent sources, so either event
  alone must be able to rebuild it:
  - on `MpdEvent`: rebuild everything as today, then carry the previous
    model's `album` field onto the new model;
  - on `UserEvent::Album`: replace only `model.album` and re-render.

  `MenuModel` is already fully owned data (`String`, `Vec<String>`,
  `Option<CopyRow>`, `Option<AlbumJump>`) so holding it across event-loop
  iterations needs no lifetime work.
- On every `MpdEvent`, send `SetCurrentAlbum(album_key(url).map(str::to_string))`
  to the worker.
- Dispatch the two new id families to `AlbumRequest`s.

### 7.5 `crates/controller/src/config.rs`

`Config::db_path` is `Option<PathBuf>`, but `load()` cannot return a `Config`
where it is `None` — `validate_db_path` turns that into a `ConfigError` first.
Now that something actually consumes it, **make the invariant type-level**:
restructure so `load()` returns a `Config` with `db_path: PathBuf`, with
`resolve()` keeping the `Option` internally.

If that restructure looks larger than it's worth in review, the fallback is an
`.expect()` whose message states the invariant and names `validate_db_path` as
what guarantees it — but prefer the type change.

Also delete the now-false comment on the field: *"Not yet read by anything —
the menu-bar app doesn't consume the album database yet"*.

---

## 8. Error handling

Album data is a **secondary feature**. A database problem must never affect
playback control — play/pause, next, clear, album navigation, the copy rows
and the playlist submenus all keep working regardless.

| Condition | Menu | Log |
|---|---|---|
| `albums.db` missing at **startup** | ⚠ tray icon, error menu, MPD not attempted — *existing behaviour, unchanged* | `error` |
| `albums.db` unreadable at **runtime** | disabled row `album database unavailable`; no summary, no `Album ▸` | `warn` with the full error |
| database recovers | row clears itself on the next successful poll (≤3s) | `info` |
| no row for the current path | summary row `not in album database`; no `Album ▸` | nothing — not an error |
| nothing playing / library-root file | no album rows at all | nothing |
| a write fails | disabled row `album database write failed`, until the next successful operation; **plus** the re-read leaves the tick unchanged | `warn` with the full error |

The distinction between "database unavailable" and "not in album database"
is the point of showing a notice at all — otherwise a broken feature and an
unknown album look identical.

---

## 9. Threading and concurrency notes

- `rusqlite::Connection` is `Send` but not `Sync`. It is created **inside** the
  worker thread and never leaves it. Do not move the open connection across
  threads.
- The worker exits when the request channel disconnects, i.e. when the event
  loop is dropped at quit. No explicit shutdown message is needed.
- The menu cannot change while displayed (§2). Events arriving during the
  modal tracking loop are queued and applied on close. Do not attempt to work
  around this.
- The worker never holds a transaction open between operations, so it does not
  block `album-tool`.

---

## 10. Testing

Follows the repo's existing split: pure model logic is unit-tested, GUI and
I/O wiring is verified by running the app. `docs/architecture.md` already
explains why there is no GUI test harness — `muda::Menu::new` panics off the
main thread and `cargo test` runs tests on spawned threads.

**Unit tests, pure, in `menu.rs`**

- `year_of` — `YYYY`, `YYYY-MM`, `YYYY-MM-DD`, `original_date` absent (falls
  back to `date`), both absent, and a non-numeric leading four characters.
- `stars` — 0 through 5.
- `album_summary` — the full worked-example table in §4.1, including the
  omit-the-whole-row case and `rating = 0` rendering as `☆☆☆☆☆` rather than
  vanishing.
- `album_section` — each `AlbumState` variant maps to the right combination of
  summary / notice / controls; `Found` yields controls, the other three do not.
- `AlbumControls` with `rating: None` leaves every rating item unchecked, and
  with `rating: Some(n)` checks exactly the one matching item.
- `strip_rating_id` / `strip_flag_id` — valid ids, malformed payloads, and ids
  belonging to the other prefixes.
- `MenuModel::show_track_section` with only album data present.

**Unit tests, in-memory database, in `albumdb`**

Matching the existing `queries.rs` test style:

- `set_flag` writes each of the four columns, both directions.
- `set_rating(Some(n))` sets `rating` and `rated_at` **and** clears
  `needs_relisten`.
- `set_rating(None)` clears `rating`, leaves `rated_at`, and leaves
  `needs_relisten` alone. (Unreachable from the menu, but still part of
  `albumdb`'s API — keep it covered.)
- `open_existing` succeeds on a database created by `open`, and **errors**
  rather than creating a file when the path does not exist — verify no file
  appears afterwards.

**Not unit-tested; document it in the module headers**

`render()`, the album worker thread, and the `data_version` poll.

**Manual verification checklist**

1. Play an album that has a rating and a year; confirm the summary row.
2. Play an album with no rating, year or flags; confirm no summary row appears.
3. Set a rating from the menu; reopen; confirm the tick moved. Confirm with
   `sqlite3 <music_dir>/albums.db 'select rating, rated_at, needs_relisten …'`.
4. Tick `Relisten`, then set a rating; confirm the relisten tick is gone
   (the accepted tradeoff in §5.1).
5. Toggle each of `Classical`, `Spoken`, `Soundtrack`; confirm in SQL.
6. Run `album-tool scan` while the app is running; confirm the menu reflects
   any change within ~3s.
7. Edit a rating with `sqlite3` directly; confirm the menu follows within ~3s.
8. `chmod 000 albums.db` for a few seconds — confirm the
   `album database unavailable` row appears and then clears itself once
   permissions are restored, and that playback controls kept working
   throughout.
9. Switch the database to WAL from another tool
   (`sqlite3 albums.db 'PRAGMA journal_mode=WAL;'`), edit a rating there, and
   confirm the menu still follows within ~3s — this is the case `stat()`
   polling would have missed. Then restart the app with nothing else holding
   the file and confirm `albums.db-wal` is gone and the mode is back to
   `delete`.
10. Play a track at the library root (no parent directory); confirm no album
    rows and no panic.

---

## 11. Documentation to update

- `README.md` — "the menu-bar app doesn't read it yet" is now false. Add the
  album display/editing to the feature list, and note the one accepted
  limitation from §6.3: **restart the app after restoring `albums.db` from a
  backup.**
- `docs/album-database.md` — "Consumed by hand in SQL, by agent skills, and
  eventually by the menu-bar app" needs updating. Add a section covering the
  app's read/write path, that it uses `open_existing` and never runs the
  schema, and the `set_rating` → `needs_relisten` interaction.
  *(The `journal_mode` section and the new "If a GUI tool switches the database
  to WAL" subsection were rewritten alongside this spec — already done.)*
- `docs/architecture.md` — add the album-db thread to the module breakdown and
  to the threading-model diagram.
- `docs/usage.md` — extend the menu walkthrough; state the rating/relisten
  interaction explicitly so it reads as intended behaviour.
- `docs/future_expansions.md` — drop whatever says the app doesn't read the
  database; add the deferred `mpdcontrol://` album verbs (§12).
- `crates/controller/src/config.rs` — the stale field comment (§7.5).
- `album-ui-goals.md` — superseded by this spec; delete once it lands.

---

## 12. Deferred

- **`mpdcontrol://` verbs for rating and flagging.** Deliberately not in this
  change, to keep it to what was asked and to avoid a second code path writing
  to the database. Note when adding them: `command::dispatch` falls through to
  shelling out to `mpc` for anything it does not recognise, so new verbs must
  be matched in `parse` *before* the fallback, or `mpc rate 5` gets run
  instead. A path-taking variant deserves particular thought — the README
  already warns the URL scheme has no allow-list, and unlike playback commands
  a rating overwrite is neither noisy nor reversible.
- Showing other columns in the submenu (`primary_genre`, `tag_state`,
  `mb_sync_state`, `notes`, `original_tracks`). The data is there and the
  submenu now exists; not requested.
- Detecting `albums.db` being replaced underneath a live connection (§6.3).
  Only reachable by restoring from backup; the remedy is to restart the app.
  If it ever needs covering, an inode check layered on top of the
  `data_version` poll is a few lines — not the whole file-metadata scheme the
  earlier draft carried.

