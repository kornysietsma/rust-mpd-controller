use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, named_params, params};
use time::OffsetDateTime;

use crate::album::{Album, AlbumFacts};
use crate::clock::format_timestamp;
use crate::enums::{GenreSource, MatchPolicy};
use crate::track_range::TrackRange;

/// Insert a new album row, or update an existing one at the same `path`.
///
/// Writes **only** the derived columns carried by `AlbumFacts`, plus
/// `status` (always set to `present` — a row that exists in MPD is by
/// definition present) and `first_seen`/`last_seen`. Every user-editable
/// column (§4.2) is absent from both the `INSERT` column list and the
/// `ON CONFLICT` `SET` clause, so it is structurally impossible for this
/// function to touch them: a fresh row gets their schema defaults, an
/// existing row keeps whatever a person put there. Returns the `album_id`.
///
/// `title`/`artist`/`title_source` are the one exception written here that
/// still needs a guard: if the existing row's `title_source` is already
/// `'manual'`, a scan must never overwrite a hand-entered title (spec
/// §5.3), so those three columns fall back to their current value in that
/// case via `CASE WHEN`.
pub fn upsert_album(conn: &Connection, facts: &AlbumFacts, now: OffsetDateTime) -> Result<i64> {
    let now = format_timestamp(now);
    conn.query_row(
        "INSERT INTO albums (
            path, status, first_seen, last_seen,
            title, artist, title_source, date, original_date, label,
            track_count, disc_count,
            mb_release_id, mb_release_group_id, mb_albumartist_id,
            tag_state
        ) VALUES (
            :path, 'present', :now, :now,
            :title, :artist, :title_source, :date, :original_date, :label,
            :track_count, :disc_count,
            :mb_release_id, :mb_release_group_id, :mb_albumartist_id,
            :tag_state
        )
        ON CONFLICT(path) DO UPDATE SET
            status               = 'present',
            last_seen            = :now,
            title                = CASE WHEN title_source = 'manual' THEN title ELSE :title END,
            artist               = CASE WHEN title_source = 'manual' THEN artist ELSE :artist END,
            title_source         = CASE WHEN title_source = 'manual' THEN title_source ELSE :title_source END,
            date                 = :date,
            original_date        = :original_date,
            label                = :label,
            track_count          = :track_count,
            disc_count           = :disc_count,
            mb_release_id        = :mb_release_id,
            mb_release_group_id  = :mb_release_group_id,
            mb_albumartist_id    = :mb_albumartist_id,
            tag_state            = :tag_state
        RETURNING album_id",
        named_params! {
            ":path": facts.path,
            ":now": now,
            ":title": facts.title,
            ":artist": facts.artist,
            ":title_source": facts.title_source.as_str(),
            ":date": facts.date,
            ":original_date": facts.original_date,
            ":label": facts.label,
            ":track_count": facts.track_count,
            ":disc_count": facts.disc_count,
            ":mb_release_id": facts.mb_release_id,
            ":mb_release_group_id": facts.mb_release_group_id,
            ":mb_albumartist_id": facts.mb_albumartist_id,
            ":tag_state": facts.tag_state.as_str(),
        },
        |row| row.get(0),
    )
    .with_context(|| format!("failed to upsert album at {:?}", facts.path))
}

/// Look up one album by its exact `path`.
pub fn find_by_path(conn: &Connection, path: &str) -> Result<Option<Album>> {
    conn.query_row(
        "SELECT * FROM albums WHERE path = ?1",
        params![path],
        Album::from_row,
    )
    .optional()
    .with_context(|| format!("failed to look up album at {path:?}"))
}

/// Every album currently `status = 'present'`.
pub fn all_present(conn: &Connection) -> Result<Vec<Album>> {
    let mut stmt = conn.prepare("SELECT * FROM albums WHERE status = 'present' ORDER BY path")?;
    let rows = stmt
        .query_map([], Album::from_row)
        .context("failed to query present albums")?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to read a present-album row")
}

/// Mark an album as no longer found in MPD. Never deletes the row — that
/// would silently discard a rating (spec §5.2). Does not touch `last_seen`:
/// that column records when the album was last *present*, and a directory
/// going missing is the opposite of that.
pub fn mark_missing(conn: &Connection, album_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE albums SET status = 'missing' WHERE album_id = ?1",
        params![album_id],
    )
    .with_context(|| format!("failed to mark album {album_id} missing"))?;
    Ok(())
}

/// One genre with its MusicBrainz vote count (or `0` for a file-tag genre,
/// which carries no vote).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenreEntry {
    pub genre: String,
    pub votes: i64,
}

/// Replace every `album_genres` row for `album_id` from the given `source`
/// with `genres`. Rows from the *other* source are untouched — a `sync` run
/// replacing `mb` genres never disturbs the `tag` genre written by `scan`,
/// and vice versa (spec §7.3).
pub fn replace_genres(
    conn: &mut Connection,
    album_id: i64,
    source: GenreSource,
    genres: &[GenreEntry],
) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM album_genres WHERE album_id = ?1 AND source = ?2",
        params![album_id, source.as_str()],
    )
    .with_context(|| format!("failed to clear existing {source:?} genres for album {album_id}"))?;
    for entry in genres {
        tx.execute(
            "INSERT INTO album_genres (album_id, genre, votes, source) VALUES (?1, ?2, ?3, ?4)",
            params![album_id, entry.genre, entry.votes, source.as_str()],
        )
        .with_context(|| {
            format!(
                "failed to insert genre {:?} for album {album_id}",
                entry.genre
            )
        })?;
    }
    tx.commit().context("failed to commit genre replacement")?;
    Ok(())
}

/// Set (or clear) the rating. Setting a numeric rating stamps `rated_at`;
/// clearing it (`None`) leaves `rated_at` as a record of when it was last
/// rated.
pub fn set_rating(
    conn: &Connection,
    album_id: i64,
    rating: Option<u8>,
    now: OffsetDateTime,
) -> Result<()> {
    match rating {
        Some(value) => conn.execute(
            "UPDATE albums SET rating = ?1, rated_at = ?2 WHERE album_id = ?3",
            params![value, format_timestamp(now), album_id],
        ),
        None => conn.execute(
            "UPDATE albums SET rating = NULL WHERE album_id = ?1",
            params![album_id],
        ),
    }
    .with_context(|| format!("failed to set rating for album {album_id}"))?;
    Ok(())
}

pub fn set_needs_relisten(conn: &Connection, album_id: i64, value: bool) -> Result<()> {
    conn.execute(
        "UPDATE albums SET needs_relisten = ?1 WHERE album_id = ?2",
        params![value, album_id],
    )
    .with_context(|| format!("failed to set needs_relisten for album {album_id}"))?;
    Ok(())
}

pub fn set_is_classical(conn: &Connection, album_id: i64, value: bool) -> Result<()> {
    conn.execute(
        "UPDATE albums SET is_classical = ?1 WHERE album_id = ?2",
        params![value, album_id],
    )
    .with_context(|| format!("failed to set is_classical for album {album_id}"))?;
    Ok(())
}

pub fn set_is_spoken(conn: &Connection, album_id: i64, value: bool) -> Result<()> {
    conn.execute(
        "UPDATE albums SET is_spoken = ?1 WHERE album_id = ?2",
        params![value, album_id],
    )
    .with_context(|| format!("failed to set is_spoken for album {album_id}"))?;
    Ok(())
}

pub fn set_is_soundtrack(conn: &Connection, album_id: i64, value: bool) -> Result<()> {
    conn.execute(
        "UPDATE albums SET is_soundtrack = ?1 WHERE album_id = ?2",
        params![value, album_id],
    )
    .with_context(|| format!("failed to set is_soundtrack for album {album_id}"))?;
    Ok(())
}

/// Set `original_tracks` by hand. Always stamps `original_tracks_src =
/// 'manual'` — the one exception, `sync` filling in an MB-derived range when
/// the column is still unset, is `sync`'s own narrow write path (spec
/// §7.3), not this general setter.
pub fn set_original_tracks(
    conn: &Connection,
    album_id: i64,
    range: Option<&TrackRange>,
) -> Result<()> {
    conn.execute(
        "UPDATE albums SET original_tracks = ?1, original_tracks_src = 'manual' WHERE album_id = ?2",
        params![range.map(|r| r.to_string()), album_id],
    )
    .with_context(|| format!("failed to set original_tracks for album {album_id}"))?;
    Ok(())
}

pub fn set_match_policy(conn: &Connection, album_id: i64, policy: MatchPolicy) -> Result<()> {
    conn.execute(
        "UPDATE albums SET match_policy = ?1 WHERE album_id = ?2",
        params![policy.as_str(), album_id],
    )
    .with_context(|| format!("failed to set match_policy for album {album_id}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::open_in_memory;
    use crate::enums::TagState;
    use crate::enums::TitleSource;
    use pretty_assertions::assert_eq;
    use time::macros::datetime;

    fn facts(path: &str) -> AlbumFacts {
        AlbumFacts {
            path: path.to_string(),
            title: Some("Some Album".to_string()),
            artist: Some("Some Artist".to_string()),
            title_source: TitleSource::Tag,
            date: Some("1999".to_string()),
            original_date: Some("1999".to_string()),
            label: Some("Some Label".to_string()),
            track_count: 10,
            disc_count: 1,
            mb_release_id: Some("mb-release-1".to_string()),
            mb_release_group_id: Some("mb-rg-1".to_string()),
            mb_albumartist_id: Some("mb-artist-1".to_string()),
            tag_state: TagState::Clean,
        }
    }

    #[test]
    fn upsert_inserts_a_new_present_album() {
        let conn = open_in_memory().unwrap();
        let now = datetime!(2026-08-05 12:00:00 UTC);

        let album_id = upsert_album(&conn, &facts("Albums/A/Artist/Album"), now).unwrap();

        let album = find_by_path(&conn, "Albums/A/Artist/Album")
            .unwrap()
            .unwrap();
        assert_eq!(album.album_id, album_id);
        assert_eq!(album.status.as_str(), "present");
        assert_eq!(album.first_seen, "2026-08-05T12:00:00Z");
        assert_eq!(album.last_seen, "2026-08-05T12:00:00Z");
        assert_eq!(album.title.as_deref(), Some("Some Album"));
        assert_eq!(album.tag_state, TagState::Clean);
    }

    /// The load-bearing invariant (plan Step 5): every user-editable column
    /// must survive a re-`upsert_album` with completely different derived
    /// data, whether the row started present or missing.
    #[test]
    fn upsert_never_touches_user_editable_columns() {
        let conn = open_in_memory().unwrap();
        let t1 = datetime!(2026-08-01 00:00:00 UTC);
        let t2 = datetime!(2026-08-05 00:00:00 UTC);

        let album_id = upsert_album(&conn, &facts("Albums/A/Artist/Album"), t1).unwrap();

        set_rating(&conn, album_id, Some(5), t1).unwrap();
        set_needs_relisten(&conn, album_id, true).unwrap();
        set_is_classical(&conn, album_id, true).unwrap();
        set_is_spoken(&conn, album_id, true).unwrap();
        set_is_soundtrack(&conn, album_id, true).unwrap();
        set_original_tracks(&conn, album_id, Some(&"1-8".parse().unwrap())).unwrap();
        set_match_policy(&conn, album_id, MatchPolicy::Never).unwrap();
        conn.execute(
            "UPDATE albums SET notes = 'hand-written note' WHERE album_id = ?1",
            params![album_id],
        )
        .unwrap();

        let before = find_by_path(&conn, "Albums/A/Artist/Album")
            .unwrap()
            .unwrap();

        let mut different_facts = facts("Albums/A/Artist/Album");
        different_facts.title = Some("A Completely Different Title".to_string());
        different_facts.artist = Some("A Different Artist".to_string());
        different_facts.tag_state = TagState::NoMbidVaried;
        different_facts.mb_release_id = None;
        different_facts.track_count = 3;
        upsert_album(&conn, &different_facts, t2).unwrap();

        let after = find_by_path(&conn, "Albums/A/Artist/Album")
            .unwrap()
            .unwrap();

        assert_eq!(after.rating, before.rating);
        assert_eq!(after.needs_relisten, before.needs_relisten);
        assert_eq!(after.rated_at, before.rated_at);
        assert_eq!(after.is_classical, before.is_classical);
        assert_eq!(after.is_spoken, before.is_spoken);
        assert_eq!(after.is_soundtrack, before.is_soundtrack);
        assert_eq!(after.original_tracks, before.original_tracks);
        assert_eq!(after.original_tracks_src, before.original_tracks_src);
        assert_eq!(after.match_policy, before.match_policy);
        assert_eq!(after.notes, before.notes);

        // The derived fields *did* change, proving the second upsert was a
        // real update and the invariant above isn't just "nothing happened".
        assert_eq!(after.title.as_deref(), Some("A Completely Different Title"));
        assert_eq!(after.tag_state, TagState::NoMbidVaried);
        assert_eq!(after.last_seen, "2026-08-05T00:00:00Z");
        assert_eq!(after.first_seen, before.first_seen);
    }

    /// Spec §5.3: a hand-entered title survives a scan. `title_source =
    /// 'manual'` is not itself user-editable in the schema's "never
    /// machine-written" sense (it's set by a future manual-edit command, not
    /// listed under §4.2's user-editable block), but the same non-negotiable
    /// invariant applies to it once set.
    #[test]
    fn upsert_never_overwrites_a_manual_title() {
        let conn = open_in_memory().unwrap();
        let t1 = datetime!(2026-08-01 00:00:00 UTC);
        let t2 = datetime!(2026-08-05 00:00:00 UTC);

        upsert_album(&conn, &facts("Albums/A/Artist/Album"), t1).unwrap();
        conn.execute(
            "UPDATE albums SET title = 'Hand-Entered Title', artist = 'Hand-Entered Artist', title_source = 'manual' WHERE path = 'Albums/A/Artist/Album'",
            [],
        )
        .unwrap();

        let mut different_facts = facts("Albums/A/Artist/Album");
        different_facts.title = Some("Scanned Title".to_string());
        different_facts.artist = Some("Scanned Artist".to_string());
        upsert_album(&conn, &different_facts, t2).unwrap();

        let after = find_by_path(&conn, "Albums/A/Artist/Album")
            .unwrap()
            .unwrap();
        assert_eq!(after.title.as_deref(), Some("Hand-Entered Title"));
        assert_eq!(after.artist.as_deref(), Some("Hand-Entered Artist"));
        assert_eq!(after.title_source, Some(TitleSource::Manual));
        // Other derived fields still update normally.
        assert_eq!(after.last_seen, "2026-08-05T00:00:00Z");
    }

    #[test]
    fn mark_missing_then_reupsert_returns_to_present_with_rating_intact() {
        let conn = open_in_memory().unwrap();
        let t1 = datetime!(2026-08-01 00:00:00 UTC);
        let t2 = datetime!(2026-08-05 00:00:00 UTC);

        let album_id = upsert_album(&conn, &facts("Albums/A/Artist/Album"), t1).unwrap();
        set_rating(&conn, album_id, Some(4), t1).unwrap();

        mark_missing(&conn, album_id).unwrap();
        let missing = find_by_path(&conn, "Albums/A/Artist/Album")
            .unwrap()
            .unwrap();
        assert_eq!(missing.status.as_str(), "missing");
        assert_eq!(missing.rating, Some(4));
        assert_eq!(missing.last_seen, "2026-08-01T00:00:00Z");

        upsert_album(&conn, &facts("Albums/A/Artist/Album"), t2).unwrap();
        let reappeared = find_by_path(&conn, "Albums/A/Artist/Album")
            .unwrap()
            .unwrap();
        assert_eq!(reappeared.status.as_str(), "present");
        assert_eq!(reappeared.rating, Some(4));
        assert_eq!(reappeared.last_seen, "2026-08-05T00:00:00Z");
    }

    #[test]
    fn find_by_path_returns_none_for_unknown_path() {
        let conn = open_in_memory().unwrap();
        assert_eq!(find_by_path(&conn, "nowhere").unwrap(), None);
    }

    #[test]
    fn all_present_excludes_missing_albums() {
        let conn = open_in_memory().unwrap();
        let now = datetime!(2026-08-05 00:00:00 UTC);
        let a = upsert_album(&conn, &facts("Albums/A/Artist/One"), now).unwrap();
        upsert_album(&conn, &facts("Albums/B/Artist/Two"), now).unwrap();
        mark_missing(&conn, a).unwrap();

        let present = all_present(&conn).unwrap();
        assert_eq!(present.len(), 1);
        assert_eq!(present[0].path, "Albums/B/Artist/Two");
    }

    #[test]
    fn replace_genres_leaves_the_other_source_alone() {
        let mut conn = open_in_memory().unwrap();
        let now = datetime!(2026-08-05 00:00:00 UTC);
        let album_id = upsert_album(&conn, &facts("Albums/A/Artist/Album"), now).unwrap();

        replace_genres(
            &mut conn,
            album_id,
            GenreSource::Tag,
            &[GenreEntry {
                genre: "Indie Rock".to_string(),
                votes: 0,
            }],
        )
        .unwrap();
        replace_genres(
            &mut conn,
            album_id,
            GenreSource::Mb,
            &[
                GenreEntry {
                    genre: "indie rock".to_string(),
                    votes: 17,
                },
                GenreEntry {
                    genre: "chamber pop".to_string(),
                    votes: 4,
                },
            ],
        )
        .unwrap();

        // Re-sync: replace `mb` again with a different set.
        replace_genres(
            &mut conn,
            album_id,
            GenreSource::Mb,
            &[GenreEntry {
                genre: "art pop".to_string(),
                votes: 2,
            }],
        )
        .unwrap();

        let mut stmt = conn
            .prepare("SELECT genre, votes, source FROM album_genres WHERE album_id = ?1 ORDER BY source, genre")
            .unwrap();
        let rows: Vec<(String, i64, String)> = stmt
            .query_map(params![album_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        assert_eq!(
            rows,
            vec![
                ("art pop".to_string(), 2, "mb".to_string()),
                ("Indie Rock".to_string(), 0, "tag".to_string()),
            ]
        );
    }

    #[test]
    fn set_rating_clears_rating_but_keeps_rated_at() {
        let conn = open_in_memory().unwrap();
        let now = datetime!(2026-08-05 00:00:00 UTC);
        let album_id = upsert_album(&conn, &facts("Albums/A/Artist/Album"), now).unwrap();

        set_rating(&conn, album_id, Some(5), now).unwrap();
        set_rating(&conn, album_id, None, now).unwrap();

        let album = find_by_path(&conn, "Albums/A/Artist/Album")
            .unwrap()
            .unwrap();
        assert_eq!(album.rating, None);
        assert_eq!(album.rated_at.as_deref(), Some("2026-08-05T00:00:00Z"));
    }

    #[test]
    fn set_original_tracks_stamps_manual_source() {
        let conn = open_in_memory().unwrap();
        let now = datetime!(2026-08-05 00:00:00 UTC);
        let album_id = upsert_album(&conn, &facts("Albums/A/Artist/Album"), now).unwrap();

        set_original_tracks(&conn, album_id, Some(&"1-10".parse().unwrap())).unwrap();

        let album = find_by_path(&conn, "Albums/A/Artist/Album")
            .unwrap()
            .unwrap();
        assert_eq!(album.original_tracks.as_deref(), Some("1-10"));
        assert_eq!(album.original_tracks_src.as_deref(), Some("manual"));
    }
}
