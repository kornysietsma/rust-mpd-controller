use std::path::Path;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OpenFlags};

const SCHEMA: &str = include_str!("../sql/schema.sql");

/// Open (creating if absent) the database at `path`, with pragmas applied and
/// the schema in place.
pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)
        .with_context(|| format!("failed to open database at {}", path.display()))?;
    init(&conn)?;
    Ok(conn)
}

/// Open an existing database read-write, with pragmas applied and **without**
/// running the schema. For consumers that must never create or migrate the
/// file — the menu-bar app, which would otherwise silently create an empty
/// database at a mistyped path. `album-tool`, which owns the schema, keeps
/// using [`open`].
///
/// `SQLITE_OPEN_CREATE` is deliberately omitted, so a missing file errors
/// here rather than being silently created — race-free, unlike an `exists()`
/// check followed by a plain `open`.
pub fn open_existing(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .with_context(|| format!("failed to open existing database at {}", path.display()))?;
    apply_pragmas(&conn)?;
    Ok(conn)
}

/// Open an in-memory database with the schema in place, for tests.
pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory().context("failed to open in-memory database")?;
    init(&conn)?;
    Ok(conn)
}

fn init(conn: &Connection) -> Result<()> {
    apply_pragmas(conn)?;
    conn.execute_batch(SCHEMA)
        .context("failed to apply schema")?;
    Ok(())
}

fn apply_pragmas(conn: &Connection) -> Result<()> {
    // `:memory:` databases can never honour DELETE — SQLite always reports
    // `memory` back for them, regardless of what's requested (see
    // `pragmas_read_back_correctly` below) — so that's accepted too.
    let journal_mode: String = conn
        .pragma_update_and_check(None, "journal_mode", "DELETE", |row| row.get(0))
        .context("failed to set journal_mode")?;
    if !journal_mode.eq_ignore_ascii_case("delete") && !journal_mode.eq_ignore_ascii_case("memory")
    {
        bail!("expected journal_mode to be set to delete, got {journal_mode:?}");
    }
    conn.pragma_update(None, "foreign_keys", "ON")
        .context("failed to set foreign_keys")?;
    conn.pragma_update(None, "busy_timeout", 5000)
        .context("failed to set busy_timeout")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rusqlite::params;

    /// A unique, existing directory for one test's `albums.db`. Unique per
    /// process *and* per call, since these tests run concurrently.
    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "albumdb-test-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn opening_twice_is_idempotent_and_preserves_data() {
        let dir = temp_dir("idempotent");
        let db_path = dir.join("albums.db");

        {
            let conn = open(&db_path).unwrap();
            conn.execute(
                "INSERT INTO albums (path, first_seen, last_seen, track_count, tag_state)
                 VALUES ('Albums/A/Artist/Album', 'now', 'now', 10, 'clean')",
                [],
            )
            .unwrap();
        }

        let conn = open(&db_path).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM albums", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pragmas_read_back_correctly() {
        let conn = open_in_memory().unwrap();

        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode.to_lowercase(), "memory");

        let foreign_keys: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(foreign_keys, 1);

        let busy_timeout: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert_eq!(busy_timeout, 5000);
    }

    #[test]
    fn journal_mode_on_disk_is_delete() {
        let dir = temp_dir("journal");
        let db_path = dir.join("albums.db");

        let conn = open(&db_path).unwrap();
        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode.to_lowercase(), "delete");
        drop(conn);

        assert!(!dir.join("albums.db-wal").exists());
        assert!(!dir.join("albums.db-shm").exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn open_existing_succeeds_on_a_database_created_by_open() {
        let dir = temp_dir("open-existing");
        let db_path = dir.join("albums.db");

        open(&db_path).unwrap();
        let conn = open_existing(&db_path).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM albums", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn open_existing_errors_and_creates_no_file_for_a_missing_path() {
        let dir = temp_dir("open-existing-missing");
        let db_path = dir.join("albums.db");

        let result = open_existing(&db_path);
        assert!(result.is_err());
        assert!(!db_path.exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    fn insert_minimal_album(conn: &Connection, path: &str) -> i64 {
        conn.execute(
            "INSERT INTO albums (path, first_seen, last_seen, track_count, tag_state)
             VALUES (?1, 'now', 'now', 10, 'clean')",
            params![path],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn check_rejects_bad_rating() {
        let conn = open_in_memory().unwrap();
        insert_minimal_album(&conn, "Albums/A/Artist/Album");
        let result = conn.execute(
            "UPDATE albums SET rating = 6 WHERE path = 'Albums/A/Artist/Album'",
            [],
        );
        assert!(result.is_err());
    }

    #[test]
    fn check_rejects_bad_tag_state() {
        let conn = open_in_memory().unwrap();
        let result = conn.execute(
            "INSERT INTO albums (path, first_seen, last_seen, track_count, tag_state)
             VALUES ('Albums/A/Artist/Album', 'now', 'now', 10, 'nonsense')",
            [],
        );
        assert!(result.is_err());
    }

    #[test]
    fn check_rejects_bad_match_policy() {
        let conn = open_in_memory().unwrap();
        insert_minimal_album(&conn, "Albums/A/Artist/Album");
        let result = conn.execute(
            "UPDATE albums SET match_policy = 'maybe' WHERE path = 'Albums/A/Artist/Album'",
            [],
        );
        assert!(result.is_err());
    }

    #[test]
    fn cascade_delete_removes_genre_rows() {
        let conn = open_in_memory().unwrap();
        let album_id = insert_minimal_album(&conn, "Albums/A/Artist/Album");
        conn.execute(
            "INSERT INTO album_genres (album_id, genre, votes, source) VALUES (?1, 'rock', 5, 'mb')",
            params![album_id],
        )
        .unwrap();

        conn.execute("DELETE FROM albums WHERE album_id = ?1", params![album_id])
            .unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM album_genres", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
}
