# How it works

A Cargo workspace of four crates:

| Crate | Responsibility |
|---|---|
| `crates/controller` | the menu-bar app and `mpdcontrol://` URL scheme — everything below |
| `crates/albumdb` | SQLite album database: schema, domain types, pure derivation logic |
| `crates/album-tool` | the `scan`/`sync`/`status` CLI that maintains it |
| `crates/mpd-conf` | parses MPD's own `mpd.conf`; shared by both binaries |

`controller` depends on `albumdb` for exactly one thing: the menu-bar app's
album-database thread (`src/album.rs`, below), which displays and edits the
current album's row. Everything else about the album side — the schema,
`album-tool`, the derivation logic — is independent; see
[album-database.md](album-database.md) for that. The rest of this document is
about `controller`.

`mpd-conf` exists so neither binary duplicates settings MPD already owns: both
infer the port (and the album database's location) from `mpd.conf` rather than
asking to be told twice. It deliberately returns `music_directory` as an
`Option` and leaves "is that required?" to the caller — `album-tool` can't
work without it, the menu-bar app doesn't care.

One binary, one long-running process. Both entry points — the menu bar and the
URL scheme — are handled inside the same process, which owns the MPD
connection and any future in-memory state.

| Module (under `crates/controller/`) | Responsibility          |
|--------------------|--------------------------------------------------------|
| `src/main.rs`      | startup, `tao` event loop, tray icon + menu, URL handling |
| `src/mpd.rs`       | MPD connection, `idle` loop, reconnect/backoff, snapshots, queue/playlist caches |
| `src/album.rs`     | album-database worker thread: reads/writes `albums.db` via `albumdb`, polls for external changes |
| `src/command.rs`   | decoded command string → dispatch against MPD          |
| `src/menu.rs`      | pure menu model (album nav, playlist membership, copy URIs, album-database section) + thin render, title formatting/truncation |
| `src/config.rs`    | TOML config load + defaults                            |
| `src/clipboard.rs` | pasteboard write (`arboard`), for the copy-URI menu rows |

`command.rs` is the shared dispatch path used by *both* menu clicks and the
URL scheme, so there is one place that knows how to turn `add <path>`,
`toggle`, etc. into MPD operations.

## Threading model

The concurrency split exists because `tray-icon`/`muda` need the `tao` event
loop on the **main thread**, while `mpd_client` is async on **tokio**:

```mermaid
sequenceDiagram
    participant MPD
    participant Idle as Idle task (tokio)
    participant Main as Main thread (tao event loop, tray + menu)
    participant Cmd as Command task (tokio)

    loop idle loop
        Idle->>MPD: idle
        MPD-->>Idle: subsystem changed
        Idle->>MPD: fetch status + song (+ queue/playlists if that subsystem changed)
        MPD-->>Idle: data
        Idle->>Main: EventLoopProxy: snapshot + Client clone
        Main->>Main: rebuild tray title + menu
    end

    Note over Main: menu click, or mpdcontrol:// URL arrives
    Main->>Cmd: spawn(Client clone, command)
    Cmd->>MPD: dispatch command
    MPD-->>Cmd: ack
    Note over MPD,Idle: resulting state change is picked up<br/>by the next idle-loop iteration
```

1. The main thread runs the event loop and owns the tray icon and menu.
2. A background tokio task holds the MPD connection and loops on `idle`,
   getting pushed notifications when player/queue state changes.
3. On each change it fetches song + status and sends them to the main thread
   via an `EventLoopProxy`, along with a cheap clone of the `Client` — so the
   main thread always holds a live handle to issue commands with, and no
   separate command channel is needed.
4. Commands (menu click or URL) clone that `Client` and run as tokio tasks, so
   execution never blocks the event loop. If MPD is disconnected, the action
   is a logged no-op rather than an error. The one exception is the copy-URI
   rows: no MPD involved, and a pasteboard write is fast enough to do inline
   on the main thread.

The title and menu refresh **only when MPD state actually changes** —
idle-driven, never polled.

### A third thread: the album-db worker

`src/album.rs` runs on its own plain `std::thread` — not a tokio task, and
independent of the MPD idle loop above. Two reasons it's separate rather than
folded into the tokio runtime:

- It's blocking: `rusqlite::Connection` is synchronous, and `albumdb` sets a
  5-second `busy_timeout` for contention with `album-tool`. A blocking call on
  a tokio task would stall the runtime; on a dedicated thread it can never
  touch the main thread's event loop.
- The connection is `Send` but not `Sync`, so it's opened inside the thread
  and never leaves it — no `Arc<Mutex<..>>` needed, since nothing else holds
  a reference to it.

It talks to the main thread the same way the MPD idle task does — an
`EventLoopProxy` out, a channel in (`std::sync::mpsc` here, since requests are
simple enums, not futures) — polling `PRAGMA data_version` every 3 seconds to
notice edits from `album-tool`, a GUI tool, or hand SQL, and responding to
`SetCurrentAlbum`/`SetRating`/`SetFlag` requests immediately rather than
waiting for the next poll tick. The write semantics and the reasoning behind
the poll interval and pragma choice are in
[album-database.md](album-database.md#the-menu-bar-apps-readwrite-path).

The main thread holds the latest `MenuModel` across iterations so either
source can rebuild the menu independently: an `MpdEvent` rebuilds everything
but carries the previous `album_section` forward, and an album-db event
replaces only `album_section` and re-renders — neither has to wait on the
other.

## Why there's no GUI test harness

Anything touching `muda` is structurally untestable — `muda::Menu` can only
be built on the main thread, and `cargo test` runs each test on a spawned
thread. That's why menu *decisions* (which sections appear, what each label
says, which song a jump targets) live in `menu::model` — pure functions over
plain primitives, fully unit tested — while `menu::render` is thin,
decision-free item construction that just isn't covered by `cargo test`.
Those paths, and everything needing a live MPD, were verified by hand instead
(see [implementation-details.md](implementation-details.md)).
