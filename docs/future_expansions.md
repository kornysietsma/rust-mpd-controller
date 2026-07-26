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

## If a CLI is ever genuinely needed

Recorded only so the door isn't accidentally closed. There is deliberately no
CLI today: `mpc` already covers terminal use, and the URL scheme covers
programmatic commands.

- A CLI process would be separate from the running service and could not see
  its in-memory state directly. For pure MPD passthrough it could just talk to
  MPD directly, like `mpc` does.
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
