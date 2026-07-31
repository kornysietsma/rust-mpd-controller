# Using it

The menu bar shows the current track as `Title - Artist` (truncated — the
dropdown's track row always shows it in full), `♫` when connected but
nothing is playing, or `🔇` when MPD is unreachable — the app keeps retrying
with backoff and recovers automatically, no restart needed. When playback is
paused, the title is prefixed with `⏸ ` (outside the truncation budget, so the
track text truncates the same whether playing or paused).

Dropdown menu, top to bottom:

- `⏯ Play/Pause`, `⏭ Next`, `⏹ Clear`
- Album navigation (only while inside a same-directory run of 5+ tracks and
  `random` is off — see [implementation-details.md](implementation-details.md)):
  - `⏮⏮ Start of Album` on track 2+ of an album, or `⏮⏮ Prev Album: <name>` on
    track 1
  - `⏭⏭ Next Album: <name>`, omitted at the last album
- `⧉ <Title - Artist>` and `⧉ <Album>` — the current track and its album, in
  full. Clicking either copies an `mpdcontrol://add "<path>"` URI to the
  clipboard, ready to paste into Obsidian or anywhere else that takes a link:
  the track's own file for the first, its parent directory (the whole album,
  since `add` is recursive) for the second. The album row is omitted for a
  library-root file, which has no album directory.
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
