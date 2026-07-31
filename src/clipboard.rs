use anyhow::{Context, Result};
use arboard::Clipboard;

/// Puts `text` on the system clipboard.
///
/// Synchronous, and called straight from the menu-event handler on the main
/// thread: this is a direct `NSPasteboard` write, not a process spawn, so
/// there's nothing worth moving onto the tokio runtime — and AppKit is
/// happiest on the main thread anyway.
///
/// A fresh `Clipboard` per call, because on macOS that's just
/// `generalPasteboard` — nothing to cache, and nothing to keep alive
/// afterwards. The pasteboard belongs to the window server, so the text
/// outlives this process (`arboard`'s "clipboard empties when the owning
/// process exits" caveat is X11-only).
pub fn copy(text: &str) -> Result<()> {
    let mut clipboard = Clipboard::new().context("failed to open the system clipboard")?;
    clipboard
        .set_text(text)
        .context("failed to write to the system clipboard")
}
