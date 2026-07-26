# Implementation plan: playlist membership, album traversal, unicode icons

Source spec: `spec.md`. This plan sequenced the work by code layer (config →
mpd.rs → menu.rs → main.rs → command.rs → docs), per `AGENTS.md`'s
personal-tool-sized scope: small steps, pure logic kept unit-testable, no
speculative abstraction.

**Status: steps 1–11 done. Next: step 12 (documentation) — the only step
left.**

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

## 12. Documentation

The only remaining step. Read-through only; no automated check.

- [ ] `README.md` — document the new menu items (album nav, In playlists, Add
      to), the `favourites` → `key_playlists` config change, icon glyphs.
- [ ] `docs/future_expansions.md` — remove "Playlist membership + Add to",
      "Favourites action", "Album traversal", "In-memory library cache" from
      the deferred list.
- [ ] `AGENTS.md` — add three gotchas to "Checking crate APIs" (all detailed
      in spec.md §2.1, with rationale — don't re-derive, just port over):
      the `mpc lsplaylists` duplication artifact, a single playlist listing
      the same song twice (needs dedup, unlike the `lsplaylists` case), and
      `Subsystem::Options` being where `random`/`repeat`/etc. changes are
      reported (not `Player`). Also add the `listplaylist` hand-roll
      (already implemented in `mpd.rs`) to the same section, parallel to the
      existing `AddRecursive` note.
- [ ] `docs/implementation-details.md` — optionally add a short note on the
      `PlayerSnapshot`/`PlaylistCache` split and why the merge lives in
      `mpd.rs`, matching the file's existing "notes for humans" style. Not
      required by the spec but keeps the file's stated purpose intact.

**Verify:** Read-through only; no automated check. Once done, this feature
branch is complete — no step 13.
