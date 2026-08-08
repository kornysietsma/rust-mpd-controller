mod album;
mod album_path;
mod clock;
mod connection;
mod date;
mod enums;
mod queries;
mod song_tags;
mod tag_state;
mod track_range;

pub use album::{Album, AlbumFacts};
pub use album_path::parse_album_path;
pub use clock::{Clock, FixedClock, SystemClock, format_timestamp};
pub use connection::{open, open_in_memory};
pub use date::normalise_date;
pub use enums::{
    AlbumStatus, GenreSource, MatchPolicy, SyncState, TagState, TitleSource, VerifyState,
};
pub use queries::{
    GenreEntry, all_albums, all_present, find_by_path, mark_missing, pending_sync_albums,
    replace_genres, set_is_classical, set_is_soundtrack, set_is_spoken, set_match_policy,
    set_needs_relisten, set_original_tracks, set_rating, set_sync_failed, set_sync_ok,
    upsert_album,
};
pub use rusqlite::Connection;
pub use song_tags::SongTags;
pub use tag_state::derive_tag_state;
pub use track_range::TrackRange;
