/// Tags read from MPD for a single track.
///
/// This is the boundary between the MPD I/O layer and every pure derivation
/// in this crate (grouping, `tag_state`, title/artist resolution) — see
/// `AGENTS.md` on why `mpd_client`'s own types can't fill this role.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SongTags {
    pub path: String,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub track: Option<String>,
    pub date: Option<String>,
    pub original_date: Option<String>,
    pub genre: Option<String>,
    pub label: Option<String>,
    pub disc: Option<String>,
    pub mb_release_id: Option<String>,
    pub mb_release_group_id: Option<String>,
    pub mb_track_id: Option<String>,
    pub mb_albumartist_id: Option<String>,
}

#[cfg(test)]
/// Test helper: a `SongTags` with only `path` set, everything else
/// defaulted. Individual fields are overridden with struct-update syntax at
/// each call site.
pub(crate) fn song(path: &str) -> SongTags {
    SongTags {
        path: path.to_string(),
        ..Default::default()
    }
}
