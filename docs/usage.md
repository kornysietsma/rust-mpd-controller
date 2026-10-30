# Using it

By default the menu bar shows just the play state — `▶`, `⏸` or `⏹` — with
the track itself as the first row of the dropdown. Set `full_title = true` in
`config.toml` to show the current track as `Title - Artist` instead
(truncated — the dropdown's track row always shows it in full), prefixed with
`⏸ ` while paused (outside the truncation budget, so the track text truncates
the same whether playing or paused). It's opt-in because on a MacBook with a
notch, macOS silently hides menu-bar items that don't fit beside it — see
[implementation-details.md](implementation-details.md). Either way, it's `♫`
when connected but nothing is queued, or `🔇` when MPD is unreachable — the
app keeps retrying with backoff and recovers automatically, no restart
needed.

Dropdown menu, top to bottom:

- `⧉ <Title - Artist>` — the current track, in full. Clicking it copies an
  `mpdcontrol://add "<path>"` URI for the track's file, same as the album row
  below.
- `⏯ Play/Pause`, `⏭ Next`, `⏹ Clear`
- Album navigation (only while inside a same-directory run of 5+ tracks and
  `random` is off — see [implementation-details.md](implementation-details.md)):
  - `⏮⏮ Start of Album` on track 2+ of an album, or `⏮⏮ Prev Album: <name>` on
    track 1
  - `⏭⏭ Next Album: <name>`, omitted at the last album
- `⧉ <Album>` — the current album, in full. Clicking it copies an
  `mpdcontrol://add "<path>"` URI to the clipboard, ready to paste into
  Obsidian or anywhere else that takes a link — the track's parent directory,
  i.e. the whole album, since `add` is recursive. Omitted for a library-root
  file, which has no album directory.
- The current album's row from `albums.db` (see
  [album-database.md](album-database.md)) — a disabled summary line, e.g.
  `1996 · ★★★★☆ · ↻ · Soundtrack`, joining whichever of year / star rating /
  `↻` (needs relisten) / category words are present; omitted entirely for an
  unrated, undated, unflagged album rather than shown empty. If the database
  can't be read, or has no row for the current path, a one-line notice takes
  its place instead (`album database unavailable` / `not in album database`).
- `Album ▶` — only when there's a database row to edit:
  - `Rating ▶` — six items, `☆☆☆☆☆ 0` through `★★★★★ 5`, ticking the stored
    value (none ticked if unrated). **There's no way to clear a rating back to
    unrated from here** — once judged, `Relisten` is how you mark "rated, but
    revisit it", not a way back to "not yet judged". **Setting a rating also
    clears `Relisten`** in the same write, on the theory that rating something
    is itself the decision `Relisten` was flagging as pending. If you want
    both — a fresh rating *and* `Relisten` still ticked — set the rating
    first and tick `Relisten` second; ticking it first and then rating loses
    the tick. (Every click closes the menu regardless, so either order costs
    two menu-opens; this just avoids losing data on the more common one.)
  - A separator, then four checkable flags: `Relisten`, `Classical`, `Spoken`,
    `Soundtrack`. Each writes its own column immediately on click.

  Every click here re-reads the row and rebuilds the menu from what's
  actually stored — so a write that silently failed shows up as "the tick I
  clicked isn't there", with a `album database write failed` notice above it,
  rather than a misleading optimistic checkmark.
- `In playlists ▶` — every playlist containing the current song, disabled
  labels, informational only; omitted when the song is in none
- `Add to ▶` — `key_playlists` minus any already containing the song; clicking
  adds it via `playlistadd`; omitted when `key_playlists` is empty or already
  covers the song
- `Quit`

Separators between sections are omitted when the section between them is
empty.

## URL scheme

`mpdcontrol://<command>` dispatches a decoded, `shlex`-tokenised command
against MPD — fire-and-forget, no return value. For example:

```
mpdcontrol://toggle
mpdcontrol://add%20%22Album%2FPath%22
```

`add <path>` is recursive, like `mpc add`. Any command not natively
implemented (`toggle`, `next`, `clear`, `add`) is shelled out to the system
`mpc` binary verbatim, covering the long tail of MPD commands.

**One command per URI.** There's no shell in the path — unrecognized tokens
become `mpc`'s `argv` — so `clear && add "..." && play` doesn't chain, it
hands `mpc` a literal `&&` and silently does nothing useful. (The predecessor
to this app, an `mpcuri://` Platypus script, did run its payload through a
shell; links written for it need splitting up.)

The menu's `⧉` rows are the easy way to get a correctly-encoded URI for
what's playing, rather than assembling one by hand.

> **Known issue:** if the app isn't already running, macOS launches it to
> handle the link, and a URL arriving in that first instant can be dropped
> while the MPD connection is still coming up (logged as `ignoring url
> command: not connected to MPD`). Clicking again works. See
> [implementation-details.md](implementation-details.md) for the fix if it
> starts to annoy.

> **Known trade-off (accepted):** this pass-through means *any* `mpdcontrol://`
> link — including one embedded in a web page — can invoke any command,
> including the `mpc` fallback. That's fine for a single-user personal tool;
> it would need a verb allowlist if it ever became multi-user or web-exposed.
