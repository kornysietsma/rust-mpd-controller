# Implementation plan: playlist membership, album traversal, unicode icons

Source spec: `spec.md`. This plan sequenced the work by code layer (config →
mpd.rs → menu.rs → main.rs → command.rs → docs), per `AGENTS.md`'s
personal-tool-sized scope: small steps, pure logic kept unit-testable, no
speculative abstraction.

**Status: steps 1–12 done. Feature branch complete — no step 13.**

## Steps 1–11 (done)

All feature work, tests, and live manual QA are complete and merged into
`spec.md` (which now reflects the design as actually built, including two
corrections found only during live testing — see below). Full step-by-step
history lives in git log / prior versions of this file if needed; it's not
reproduced here since nothing in it is still open.

**Two real bugs were found and fixed during the live manual QA pass** (step
11), neither caught by the unit suite since both need live MPD state to
surface. Both are now reflected in `spec.md` as part of the spec itself, not
just as bug-fix trivia:

1. **Duplicate "In playlists" entries** — a playlist can legitimately list
   the same song twice (confirmed live: `Five stars.m3u` does, for Lorde's
   "Royals"); the membership cache now dedupes (spec §4.2, §2.1).
2. **`random` toggle didn't hide/show album nav** — MPD reports
   `random`/`repeat`/etc. changes on `Subsystem::Options`, not `Player`;
   `mpd.rs`'s idle-loop handler now treats `Options` the same as `Player`
   (spec §4.1, §2.1).

`cargo build`/`test` (70/70)/`clippy`/`fmt` are all clean. Manual QA covered
every checklist item that used to live in this file (playlist membership,
album traversal, icons, paused indicator, resilience, and v1 regressions) —
all confirmed working live against the real MPD server and library.

## 12. Documentation (done)

- [x] `README.md` — documented the new menu items (album nav, In playlists,
      Add to), the `favourites` → `key_playlists` config change, icon
      glyphs/paused indicator.
- [x] `docs/future_expansions.md` — removed "Playlist membership + Add to",
      "Favourites action", "Album traversal", "In-memory library cache" from
      the deferred list.
- [x] `AGENTS.md` — added the three gotchas from spec.md §2.1 (`mpc
      lsplaylists` duplication, playlist-content dedup, `Subsystem::Options`
      vs `Player`) plus the `listplaylist` hand-roll note parallel to
      `AddRecursive`.
- [x] `docs/implementation-details.md` — added a short note on the
      `PlayerSnapshot`/`PlaylistCache` split and why the merge lives in
      `mpd.rs`.

**Verify:** Read-through only; no automated check. Done — this feature
branch is complete, no step 13.
