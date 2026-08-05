use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;

const SCHEMA: &str = include_str!("../sql/schema.sql");

/// Open (creating if absent) the database at `path`, with pragmas applied and
/// the schema in place.
pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)
        .with_context(|| format!("failed to open database at {}", path.display()))?;
    init(&conn)?;
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
    conn.pragma_update(None, "journal_mode", "DELETE")
        .context("failed to set journal_mode")?;
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

    fn table_columns(conn: &Connection, table: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    #[test]
    fn fresh_db_creates_both_tables_with_every_column() {
        let conn = open_in_memory().unwrap();

        let albums = table_columns(&conn, "albums");
        for expected in [
            "album_id",
            "path",
            "status",
            "fingerprint",
            "first_seen",
            "last_seen",
            "title",
            "artist",
            "title_source",
            "date",
            "original_date",
            "label",
            "track_count",
            "disc_count",
            "mb_release_id",
            "mb_release_group_id",
            "mb_albumartist_id",
            "tag_state",
            "mb_sync_state",
            "mb_synced_at",
            "mb_sync_error",
            "mb_original_count",
            "primary_genre",
            "mb_verify_state",
            "mb_verified_at",
            "mb_verify_notes",
            "rating",
            "needs_relisten",
            "rated_at",
            "is_classical",
            "is_spoken",
            "is_soundtrack",
            "original_tracks",
            "original_tracks_src",
            "match_policy",
            "notes",
        ] {
            assert!(
                albums.contains(&expected.to_string()),
                "missing albums.{expected}"
            );
        }

        let genres = table_columns(&conn, "album_genres");
        for expected in ["album_id", "genre", "votes", "source"] {
            assert!(
                genres.contains(&expected.to_string()),
                "missing album_genres.{expected}"
            );
        }
    }

    #[test]
    fn opening_twice_is_idempotent_and_preserves_data() {
        let dir = std::env::temp_dir().join(format!(
            "albumdb-test-idempotent-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
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
        let dir = std::env::temp_dir().join(format!(
            "albumdb-test-journal-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
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
