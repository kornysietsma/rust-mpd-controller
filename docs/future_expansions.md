# Future expansions

Ideas captured so the design leaves room for them. **None of this is built or
planned** — it's here to record intent, and to avoid designing speculative
complexity into the current code.

> **Design principle — YAGNI.** The architecture is deliberately matched to
> current needs. New functionality is expected to arrive through the menu bar
> or the URL scheme; if a genuine need for anything below appears later, this
> is how it would fit.

## Deferred menu features

All of these fit naturally into the existing single stateful service — no
architectural change needed.

- **Sticker metadata** — read/write MPD's sticker database (e.g. ratings),
  surfaced through the menu and/or the URL scheme.
- **Everything that consumes the album database.** In rough order of value:
  rating the current album from the menu; showing rating and original year in
  album menu labels; "play a random 5-star album, excluding classical and
  spoken word"; playing only the original tracks via `original_tracks`. The
  database already holds all of this — see
  [album-database.md](album-database.md). Nothing in `controller` reads
  `albumdb` yet.

## Deferred album-database features

The schema already has the columns for all of these, so none needs a schema
change to pick up.

- **`album-tool verify`** — re-check tags against current MusicBrainz data and
  flag drift, merged releases and disagreements, writing `mb_verify_state`,
  `mb_verified_at`, `mb_verify_notes`. Those columns exist from day one.
- **`album-tool export`** — CSV out, for spreadsheet round-tripping.
- **Category derivation from MusicBrainz genres** — filling `is_classical` /
  `is_spoken` / `is_soundtrack` automatically. Left until the genre data could
  be inspected properly; the 69 hand-marked classical albums are already a
  better seed than any rule.
- **Automatic rename reconciliation** — matching a disappeared directory to a
  newly-appeared one by content fingerprint, so a moved album keeps its
  `album_id` and rating without manual intervention. Cut from v1: it assumed
  renames would be common enough to justify the machinery, which was never
  measured, and since disappearances are preserved rather than deleted the
  manual fix is cheap. Revisit only if `album-tool status` starts showing
  renames as a repeated nuisance.

## Deferred URL-scheme features

- **A compound "play this now" command.** The `mpcuri://` app this replaced
  ran its payload through a shell, so an Obsidian link could say
  `clear && add "<album>" && play`. This app deliberately has no shell — one
  command per URI — so that idiom is gone, and gone quietly: `mpc` just
  receives a literal `&&` and does nothing. If it turns out to be missed, the
  shape is a native `playnow <path>` in `command.rs` (clear + add + play in
  one dispatch), *not* re-introducing shell semantics into the URL payload.

## If a *controller* CLI is ever genuinely needed

Recorded only so the door isn't accidentally closed. `album-tool` is a CLI, but
it never talks to the menu-bar app — it reads MPD and writes SQLite. There is
deliberately no CLI onto the **running controller service**: `mpc` already
covers terminal playback, and the URL scheme covers programmatic commands.

- Such a CLI would be a separate process from the running service and could not
  see its in-memory state directly. For pure MPD passthrough it could just talk
  to MPD directly, like `mpc` does.
- For anything needing the service's cached state, it would connect over a
  **Unix domain socket** exposed by the service for request/response. That
  socket doesn't exist and shouldn't be built until something needs it.
- **The URL scheme is not a CLI transport** — it is one-way and returns no
  results. It's for commands, not queries.

## Deliberate non-goals

Not deferred — actively not wanted:

- A rich graphical popover / album-art panel. Native dropdown menus only; this
  is why the stack avoids Tauri.
- Cross-platform support. macOS only.
- Packaging for distribution to anyone else. Personal install only.
