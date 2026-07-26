# Spec: playlist membership, album traversal, and unicode menu icons

Extends the v1 macOS menu-bar MPD controller with features ported from the
older `mpd-xbar` widget, plus album-level queue navigation.

Scope is personal-tool-sized (see `AGENTS.md`): prefer simple working code,
don't design in speculative complexity.

## 1. Summary of features

1. **Playlist cache** — track the contents of all MPD stored playlists in
   memory.
2. **"In playlists"** — submenu listing every playlist containing the current
   song (informational only).
3. **"Add to"** — submenu adding the current song to a configured subset of
   playlists.
4. **Album traversal** — "Prev Album" / "Next Album" items naming their
   destination album, shown only when album-sequential playback is detected.
5. **Paused indicator** — `⏸` prefix on the tray title when paused.
6. **Unicode icons** — monochrome media glyphs on the playback menu items.

## 2. Empirical basis

Measured against the live library on 2026-07-27; thresholds below are derived
from this data, not guessed.

| Measure | Value |
|---|---|
| Library size | 28,928 tracks, 1,877 directories, 1,845 distinct album titles |
| Missing `album` tag | 4 tracks (0.01%) |
| Missing `albumartist` tag | 30 tracks (0.10%) |
| Stored playlists | 45, ~12,454 total entries |
| Files appearing in any playlist | 4,044 (max 9 playlists for one file) |
| Album title length | mean 20.7 chars; 159 over 40; 41 over 60; max 192 |
| `playlistinfo` cost | 847 bytes/song, 0.125 ms/song |

**Album-run distribution** — percent of queue positions sitting in a
same-directory run of at least N, which is what drives the visibility rule in
§5.3:

| Playlist | tracks | ≥2 | ≥3 | ≥5 | ≥8 |
|---|---|---|---|---|---|
| rem_all | 210 | 100% | 100% | **100%** | 100% |
| hottest_100_best | 86 | 99% | 99% | **99%** | 91% |
| mashup_albums | 244 | 98% | 98% | **97%** | 97% |
| Beatles_best | 61 | 100% | 97% | **90%** | 79% |
| classical_best | 9 | 67% | 67% | 67% | 0% |
| My Top Rated | 2734 | 54% | 37% | **18%** | 5% |
| mellow_2024 | 144 | 47% | 26% | **4%** | 0% |
| Five stars | 444 | 30% | 12% | **5%** | 3% |

A threshold of 5 separates album listening (90–100%) from track-oriented
playlists (4–18%).

### 2.1 Gotchas found while measuring

- **`mpc lsplaylists` returns each playlist twice** (once bare, once with
  `.m3u`). This is an `mpc` artifact — it also lists the playlist directory's
  files. MPD's own `listplaylists` returns 45 clean bare names. **Do not add
  deduplication**; `mpd_client` sees the clean list.
- Tags come free. MPD parses metadata at scan time; `playlistinfo` is served
  from MPD's memory. No file parsing, no per-song round trips.
- All multi-disc albums in this library live in a **single directory** with
  track numbers running continuously across discs. There are no `CD1/`/`CD2/`
  subdirectories, so directory-based album grouping handles them correctly.
- **A playlist can list the same song twice** — `Five stars.m3u` does, for
  Lorde's "Royals" (confirmed live, not a measurement artifact). Unlike the
  `lsplaylists` gotcha above, this one *does* need deduplication — see §4.2.
- **MPD reports `random`/`repeat`/etc. changes on `Subsystem::Options`, not
  `Player`** — found live when toggling `random` mid-album-run had no effect
  until `Options` was handled (§4.1).

## 3. Configuration changes

`~/.config/mpd-controller/config.toml`

```toml
mpd_host = "localhost"
mpd_port = 6600
key_playlists = ["Five stars", "My Top Rated", "mellow_2024"]
```

- **Remove the `favourites` field** from `Config` and `Default`. It was parsed
  but never used; `key_playlists` covers the same need. Serde ignores unknown
  keys by default, so an existing config still containing `favourites` will
  load without error.
- `key_playlists` keeps its current type (`Vec<String>`) and default (empty).
- Defaults must remain usable on a machine with no config file at all
  (true when this spec was written; a config now exists on this dev machine
  from manual QA), which means an empty `key_playlists` is a normal state —
  the "Add to" submenu is simply omitted (§6.2).
- Names in `key_playlists` that don't exist in MPD are logged at `warn` on
  cache load and omitted from the "Add to" submenu.

Update `config.rs` tests: drop `favourites` assertions from
`partial_file_defaults_missing_fields` and `full_file_parses_all_fields`.

## 4. Data fetched from MPD

### 4.1 Queue cache

`PlayerSnapshot` gains the queue. Because a large queue is expensive (2,734
tracks = 2.2 MiB, ~342 ms), it is **not** refetched on every event:

| Trigger | Action |
|---|---|
| On connect | fetch `Status`, `CurrentSong`, `Queue` |
| `Subsystem::Queue` | refetch all three |
| `Subsystem::Player` | refetch `Status` + `CurrentSong` only; **reuse cached queue** |
| `Subsystem::Options` | same as `Subsystem::Player` |
| `Subsystem::StoredPlaylist` | reload playlist cache only (§4.2) |
| Disconnect | drop both caches |

Queue *contents* only change on `Subsystem::Queue`; Player events change only
position and play state. This keeps track changes free.

**`Subsystem::Options` is required, not optional**: MPD reports `random`
(and `repeat`/`single`/`consume`/crossfade/replay-gain) changes on `Options`,
*not* `Player` — confirmed live when toggling `random` mid-album-run had no
effect on the menu until `Options` was handled identically to `Player`. Both
just refetch `Status`+`CurrentSong` and reuse the cached queue, since `random`
lives in `Status`.

### 4.2 Playlist cache

- On connect and on every `Subsystem::StoredPlaylist` event, reload **all**
  playlists. MPD's event doesn't say which playlist changed, and a full reload
  of 45 playlists / 12.4k entries is cheap.
- Use `listplaylist` (filenames only), **not** `listplaylistinfo` — membership
  needs only file URLs, and the tagged variant is far larger.
- Structure: `HashMap<String, Vec<String>>` mapping song URL → sorted,
  **deduplicated** playlist names.
- A single playlist can legitimately list the same song more than once —
  confirmed live: `Five stars.m3u` lists Lorde's "Royals" twice. Without
  dedup, "In playlists" showed that playlist's name twice for that song.
  Sort, then `Vec::dedup()` (which only removes *adjacent* duplicates, hence
  the sort first) when building each song's membership entry.
- Reloading on `StoredPlaylist` also covers our own writes, so an "Add to"
  click needs **no manual cache patching** — one mutation path, which cannot
  drift out of sync and cannot desync when an MPD write fails.
- A failure to load the cache is logged at `warn`; the menu then renders with
  empty membership rather than failing.

`mpd.rs`'s idle-loop event handler matches on `Subsystem::Queue`,
`Subsystem::Player`, `Subsystem::Options` (§4.1), and `Subsystem::StoredPlaylist`;
every other subsystem is ignored.

## 5. Album traversal

### 5.1 Album identity

Two queue entries are the **same album** iff they share a parent directory:

```rust
fn album_key(url: &str) -> Option<&str>   // parent path of the song URL
```

Chosen over `(album, albumartist)` tags on the evidence: the tag rule merges
`Peter Gabriel / Peter Gabriel` — genuinely **two different self-titled
albums** (`[1984]` and `[2002]`) — into one 30-track blob, and does the same to
`The Clash [1999]` / `[2002]`. Directory grouping keeps them apart, needs no
tags at all, and is correct on every multi-disc album in the library.

Known cost: 2 directories under `Unnmatched/` hold more than one album (a stray
"Brand New Year" track inside a Christmas compilation folder, and an untagged
track in a birthday mix). Accepted — 2 errors in 1,877.

### 5.2 Album display name

The `album` tag of the **first track of the target album's run**, falling back
to the directory's basename when that tag is absent. Directories map 1:1 to
albums ~99.9% of the time, so this is reliable.

Truncate the album name to **40 chars** using the existing `truncate()` helper,
applied to the name only so the `⏭⏭ Next Album: ` prefix is never pushed off.
Affects 159 of 1,845 titles.

### 5.3 Visibility rule

```
show_album_nav = !status.random && current_run_len >= 5
```

`current_run_len` is the length of the maximal contiguous same-directory run
containing the current position. Scan outward from the current position only —
O(album length), not O(queue).

MPD's `random` mode does not reorder the queue; it plays it in a random order
while `playlistinfo` order stays put, so queue-order album jumps are
meaningless there. `Status` already exposes `random`, and `PlayerSnapshot`
already carries the full `Status`.

The rule is best read as *"am I inside an album block right now"* rather than
*"is this queue album-ordered"*. When `My Top Rated` hits its 37-track album
run, showing album nav there is correct, not a false positive.

Album nav is hidden entirely when: nothing is playing, the queue is empty, MPD
is disconnected, `random` is on, or the run is shorter than 5.

### 5.4 Traversal semantics

Let the current album's run occupy `[start, end]` in queue order, with the
current position at `pos`.

- **Next Album** — the run beginning at `end + 1`, if any. Target: that run's
  **first** entry. Label: `Next Album: <name>`.
- **Prev Album / Start of Album** — two states, decided by `pos`:
  - `pos > start` (track 2 or later) → target `queue[start]`, the current
    album's first entry. Label: `Start of Album`, with no album name — so the
    40-char truncation in §5.2 doesn't apply to this branch.
  - `pos == start` (track 1) → the run ending at `start - 1`, if any. Target:
    that run's **first** entry. Label: `Prev Album: <name>`.

  Reaching the previous album from mid-album is therefore two clicks. The label
  always names where you'll land, so the item never misstates its destination
  even though its meaning shifts with position.
- Neither wraps around, regardless of `repeat` mode.
- Next Album is omitted at the last album. The prev item is omitted **only** at
  track 1 of the first album; at track 2+ of the first album it still offers
  "Start of Album".

Both states share the `album:<song_id>` id family and the same `AlbumJump`
shape — the only difference is which song id and label the model computes, so
`render()` and click dispatch need no knowledge of the distinction. The
backward scan for the previous run's start runs only in the `pos == start`
branch.

Jump via `Play::song(SongId)` using the cached `SongInQueue.id` — **not**
`SongPosition`, since positions shift if the queue is edited between the
snapshot and the click.

## 6. Menu structure

Actions first, then current-track context:

Mid-album (track 2 or later):

```
┌────────────────────────────────┐
│ ⏯  Play/Pause                  │
│ ⏭  Next                        │
│ ⏹  Clear                       │
├────────────────────────────────┤
│ ⏮⏮ Start of Album              │   (album rows optional — §5.3)
│ ⏭⏭ Next Album: Let It Be       │
├────────────────────────────────┤
│ Come Together - The Beatles    │   (disabled; only when truncated)
│ In playlists            ▶      │   (omitted when empty)
│ Add to                  ▶      │   (omitted when empty)
├────────────────────────────────┤
│ Quit                           │
└────────────────────────────────┘
```

On track 1 of an album, only the first album row differs — it becomes the
jump to the preceding album, keeping the same `⏮⏮` icon and menu position:

```
├────────────────────────────────┤
│ ⏮⏮ Prev Album: Abbey Road      │
│ ⏭⏭ Next Album: Let It Be       │
├────────────────────────────────┤
```

Separators are omitted when the section between them is empty, so the menu
never shows two adjacent separators.

### 6.1 "In playlists"

- Lists every playlist containing the current song — **all** playlists, not
  just `key_playlists`.
- Sorted alphabetically, case-insensitive.
- Items are **disabled labels** (`MenuItem::new(name, false, None)`) — purely
  informational, matching the xbar widget. No destructive action sits one slip
  away from the pointer.
- The whole submenu is omitted when the song is in no playlists.
- Bounded at 9 entries by the library data.

### 6.2 "Add to"

- Lists `key_playlists` **minus** those already containing the song (xbar
  parity — an add that would be a no-op isn't offered).
- Preserves the order given in the config file.
- Each item is enabled, with id `addto:<playlist name>`.
- Clicking issues MPD's `playlistadd <playlist> <song url>`.
- The submenu is omitted when `key_playlists` is empty (the default) or when
  every configured playlist already contains the song.
- **Feedback is implicit**: the resulting `StoredPlaylist` event reloads the
  cache and rebuilds the menu, so the playlist moves from "Add to" to "In
  playlists". No notification mechanism is added.
- Failures are logged at `warn` via the existing `spawn_command` path.

## 7. Unicode icons

All glyphs are pinned to **text presentation** with `U+FE0E` so they render
monochrome and narrow, matching the menu-bar font weight rather than turning
into wide colour emoji.

| Use | Codepoints | Rust literal |
|---|---|---|
| Play/Pause | `U+23EF` + `U+FE0E` | `"\u{23EF}\u{FE0E}"` |
| Next | `U+23ED` + `U+FE0E` | `"\u{23ED}\u{FE0E}"` |
| Clear | `U+23F9` + `U+FE0E` | `"\u{23F9}\u{FE0E}"` |
| Prev Album / Start of Album | `U+23EE` ×2 + `U+FE0E` | `"\u{23EE}\u{FE0E}\u{23EE}\u{FE0E}"` |
| Next Album | `U+23ED` ×2 + `U+FE0E` | `"\u{23ED}\u{FE0E}\u{23ED}\u{FE0E}"` |
| Paused (tray) | `U+23F8` + `U+FE0E` | `"\u{23F8}\u{FE0E}"` |

Album nav uses **doubled** arrows to distinguish it from single-track nav.
`⏩`/`⏪` were rejected: they default to colour-emoji presentation and would
rely on `U+FE0E` being honoured to look consistent.

The doubled `⏮⏮` is used for **both** states of the prev item (§5.4). The icon
deliberately does not change alongside the label, so the row doesn't appear to
flicker between two different items as you move through an album.

Existing `♫` (empty queue) and `🔇` (disconnected) are unchanged.

> **Verify visually once running.** macOS menu-bar font substitution is hard to
> predict. Per `AGENTS.md`, don't screenshot — just ask what's displayed.

### 7.1 Paused indicator

When `status.state == PlayState::Paused`, prefix the tray title with the paused
glyph and a space:

```
⏸ Come Together - The Beatl…
```

- The prefix sits **outside** the existing 25-char budget, so the track text
  truncates identically whether playing or paused.
- Applies only to `Paused`. `Stopped` shows the track with no indicator
  (preserving current behaviour); `Playing` is unchanged.
- The empty-queue and disconnected displays are unaffected.

Implementation: `track_display` gains a `state: PlayState` parameter (the
prefix is applied to the already-truncated title *inside* `track_display`,
after truncation) rather than adding a separate wrapper function — this
keeps the whole "format, truncate, prefix" sequence in one pure,
primitive-only function that stays directly unit-testable. `full_text` (the
untruncated text used by the disabled dropdown row, unrelated to the tray
title) never gets the prefix.

## 8. Code structure

### 8.1 `menu.rs` — pure model, thin render

Per the `AGENTS.md` rule about keeping display logic on plain primitives (so it
stays testable without a live connection), split menu construction in two.

```rust
/// A queue entry reduced to primitives — no mpd_client types, so the
/// model layer is unit-testable. Extracted at the call site.
pub struct QueueEntry {
    pub url: String,
    pub album: Option<String>,
    pub id: u64,           // SongInQueue.id
}

pub struct AlbumJump {
    pub label: String,     // full row text, e.g. "Prev Album: Abbey Road"
                            // or literally "Start of Album" — only the name
                            // substring inside it is 40-char truncated, so
                            // render() never branches on which case produced it
    pub song_id: u64,
}

pub struct MenuModel {
    pub full_text: Option<String>,
    pub prev_album: Option<AlbumJump>,
    pub next_album: Option<AlbumJump>,
    pub in_playlists: Vec<String>,
    pub add_to: Vec<String>,
}

/// Pure: all the rules in §5 and §6 live here.
pub fn model(
    state: PlayState,
    random: bool,
    current: Option<&QueueEntry>,
    current_pos: Option<usize>,
    queue: &[QueueEntry],
    playlists_for_url: &[String],
    key_playlists: &[String],
    title: Option<&str>,
    artists: &[String],
) -> MenuModel;

/// Thin: turns a MenuModel into muda items. No decisions here.
pub fn render(model: &MenuModel) -> Result<Menu>;
```

`MenuModel` has no `title` field (an earlier draft of this spec included one,
mirroring `Display`) — nothing in `render()` ever needs it, since the tray
title is set independently via the existing `Display`/`display_for_snapshot`
path below. Cut per YAGNI; add back only if a real consumer shows up.

`format_track`, `truncate`, `empty_queue_display` and `disconnected_display`
stay as they are.

### 8.2 Menu item ids

Static ids (`PLAY_PAUSE_ID`, `NEXT_ID`, `CLEAR_ID`, `QUIT_ID`) are unchanged.
Two dynamic id families are added, since playlist and album items can't have
compile-time constants:

- `album:<song_id>` — album jump target
- `addto:<playlist name>` — playlist to add to

Playlist names may contain `:`, so parse with `strip_prefix("addto:")` (as
§8.3's dispatch snippet does) and treat the **entire remainder** as the name
— this only strips the fixed literal prefix, so it can't be tripped up by
extra colons inside the name itself.

### 8.3 `main.rs` dispatch

Extend the existing if-chain in the `MenuEvent` arm with two prefix branches
before the fallthrough:

```rust
} else if let Some(rest) = menu_event.id.0.strip_prefix("album:") {
    // parse SongId, spawn command::play_song
} else if let Some(name) = menu_event.id.0.strip_prefix("addto:") {
    // spawn command::add_to_playlist(name, current_url)
}
```

State held across events grows from `play_state` to also include the current
song's URL (`current_url: Option<String>`), needed so an `addto:` click can
act on the song the menu was built from. **Not** the cached queue or
playlist cache — `album:<id>` needs no queue lookup (the id already names
the target `SongId` directly, which is the whole point of using `SongId`
over `SongPosition`, §5.4), and nothing else in dispatch reads either
cache. Holding them anyway would be unread state.

### 8.4 `command.rs`

Two new functions, following the existing `next`/`clear` shape:

```rust
pub async fn play_song(client: &Client, id: SongId) -> Result<()>;
pub async fn add_to_playlist(client: &Client, playlist: &str, url: &str) -> Result<()>;
```

`add_to_playlist` wraps MPD's `playlistadd`. Check whether `mpd_client` exposes
a typed command for it before hand-rolling one; if hand-rolled, follow the
`AddRecursive` pattern already in the file.

**No URL-scheme changes.** Album nav stays menu-only. Playlist adds are already
reachable via the existing `mpc` fallback (`mpc addplaylist <playlist> <uri>`),
so they need no native command either. `ParsedCommand` and `dispatch` are
untouched.

## 9. Testing

All of these run without a live MPD connection.

**Album grouping (`album_key`)**
- same directory → same album
- different directory → different album
- multi-disc album in one directory stays one album
- files at library root (no parent) handled without panic

**Run detection**
- run length at start / middle / end of a run
- single-track run → length 1
- empty queue, and a queue of exactly one track
- current position out of bounds → no nav, no panic

**Visibility rule**
- run of 5 → shown; run of 4 → hidden
- `random = true` → hidden regardless of run length
- stopped / no current song → hidden

**Traversal targets**
- next album returns the *first* entry of the following run
- on track 1, prev returns the *first* entry of the *preceding* run, labelled
  `Prev Album: <name>`
- on track 2, prev returns the current run's *first* entry, labelled
  `Start of Album`
- on the *last* track of a run, prev still returns the current run's first entry
- two clicks from track 5: first lands on track 1, second on the previous album
- track 1 of the first album → no prev item at all
- track 2+ of the first album → `Start of Album` is still offered
- last album → no next
- no wraparound when `repeat` is set

**Album labels**
- name taken from the target run's first track
- falls back to directory basename when the `album` tag is absent
- truncated at 40 chars; a 192-char title doesn't blow up the label

**Playlist membership**
- "In playlists" sorted case-insensitively
- "Add to" excludes playlists already containing the song
- empty `key_playlists` → no "Add to" submenu
- song in no playlists → no "In playlists" submenu
- `key_playlists` naming a nonexistent playlist → omitted

**Menu ids**
- `addto:` round-trips a playlist name containing a colon
- `album:` parses back to the right song id

**Paused indicator**
- `Paused` prefixes the glyph; `Playing` and `Stopped` do not
- the prefix does not consume the 25-char track budget
- long titles truncate identically whether playing or paused

## 10. Documentation to update

- `README.md` — new menu items, `favourites` removal, `key_playlists` usage.
- `docs/future_expansions.md` — remove "Playlist membership + Add to",
  "Favourites action", "Album traversal", and "In-memory library cache" from
  the deferred list; they are now built.
- `AGENTS.md` — add three gotchas (§2.1) to the "Checking crate APIs"
  section, so none is rediscovered: the `mpc lsplaylists` duplication
  artifact, a single playlist listing the same song twice (needs dedup,
  unlike the `lsplaylists` case), and `Subsystem::Options` being where
  `random`/`repeat`/etc. changes are reported (not `Player`).

## 11. Out of scope

- Removing songs from playlists (the "In playlists" submenu is inert).
- Loading/playing a playlist from the menu.
- URL-scheme commands for album navigation.
- Any notification/toast on a successful add.
