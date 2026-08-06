use std::str::FromStr;

use rusqlite::Row;

use crate::enums::{AlbumStatus, MatchPolicy, SyncState, TagState, TitleSource, VerifyState};

/// The derived facts a `scan` produces for one album directory — everything
/// `upsert_album` writes. Deliberately excludes every user-editable column
/// (§4.2): `upsert_album` must be structurally unable to touch them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumFacts {
    pub path: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub title_source: TitleSource,
    pub date: Option<String>,
    pub original_date: Option<String>,
    pub label: Option<String>,
    pub track_count: i64,
    pub disc_count: i64,
    pub mb_release_id: Option<String>,
    pub mb_release_group_id: Option<String>,
    pub mb_albumartist_id: Option<String>,
    pub tag_state: TagState,
}

/// One full `albums` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Album {
    pub album_id: i64,
    pub path: String,
    pub status: AlbumStatus,
    pub first_seen: String,
    pub last_seen: String,

    pub title: Option<String>,
    pub artist: Option<String>,
    pub title_source: Option<TitleSource>,
    pub date: Option<String>,
    pub original_date: Option<String>,
    pub label: Option<String>,
    pub track_count: i64,
    pub disc_count: i64,

    pub mb_release_id: Option<String>,
    pub mb_release_group_id: Option<String>,
    pub mb_albumartist_id: Option<String>,

    pub tag_state: TagState,

    pub mb_sync_state: SyncState,
    pub mb_synced_at: Option<String>,
    pub mb_sync_error: Option<String>,
    pub mb_original_count: Option<i64>,
    pub primary_genre: Option<String>,

    pub mb_verify_state: VerifyState,
    pub mb_verified_at: Option<String>,
    pub mb_verify_notes: Option<String>,

    pub rating: Option<i64>,
    pub needs_relisten: bool,
    pub rated_at: Option<String>,
    pub is_classical: bool,
    pub is_spoken: bool,
    pub is_soundtrack: bool,
    pub original_tracks: Option<String>,
    pub original_tracks_src: Option<String>,
    pub match_policy: MatchPolicy,
    pub notes: Option<String>,
}

fn parse_enum<T: FromStr<Err = String>>(column: &str, value: String) -> rusqlite::Result<T> {
    T::from_str(&value).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            format!("{column}: {e}").into(),
        )
    })
}

fn parse_opt_enum<T: FromStr<Err = String>>(
    column: &str,
    value: Option<String>,
) -> rusqlite::Result<Option<T>> {
    value.map(|v| parse_enum(column, v)).transpose()
}

impl Album {
    pub(crate) fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Album {
            album_id: row.get("album_id")?,
            path: row.get("path")?,
            status: parse_enum("status", row.get("status")?)?,
            first_seen: row.get("first_seen")?,
            last_seen: row.get("last_seen")?,
            title: row.get("title")?,
            artist: row.get("artist")?,
            title_source: parse_opt_enum("title_source", row.get("title_source")?)?,
            date: row.get("date")?,
            original_date: row.get("original_date")?,
            label: row.get("label")?,
            track_count: row.get("track_count")?,
            disc_count: row.get("disc_count")?,
            mb_release_id: row.get("mb_release_id")?,
            mb_release_group_id: row.get("mb_release_group_id")?,
            mb_albumartist_id: row.get("mb_albumartist_id")?,
            tag_state: parse_enum("tag_state", row.get("tag_state")?)?,
            mb_sync_state: parse_enum("mb_sync_state", row.get("mb_sync_state")?)?,
            mb_synced_at: row.get("mb_synced_at")?,
            mb_sync_error: row.get("mb_sync_error")?,
            mb_original_count: row.get("mb_original_count")?,
            primary_genre: row.get("primary_genre")?,
            mb_verify_state: parse_enum("mb_verify_state", row.get("mb_verify_state")?)?,
            mb_verified_at: row.get("mb_verified_at")?,
            mb_verify_notes: row.get("mb_verify_notes")?,
            rating: row.get("rating")?,
            needs_relisten: row.get("needs_relisten")?,
            rated_at: row.get("rated_at")?,
            is_classical: row.get("is_classical")?,
            is_spoken: row.get("is_spoken")?,
            is_soundtrack: row.get("is_soundtrack")?,
            original_tracks: row.get("original_tracks")?,
            original_tracks_src: row.get("original_tracks_src")?,
            match_policy: parse_enum("match_policy", row.get("match_policy")?)?,
            notes: row.get("notes")?,
        })
    }
}
