/// Tags read from MPD for a single track.
///
/// This is the boundary between the MPD I/O layer and every pure derivation
/// in this crate (grouping, `tag_state`, title/artist resolution) — see
/// `AGENTS.md` on why `mpd_client`'s own types can't fill this role.
/// `artist` is deliberately carried even though no derivation reads it: that
/// it is *not* consulted is the documented rule (a compilation's many track
/// artists must not make an album "varied"), and the tests that pin that rule
/// need the field to express it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SongTags {
    pub path: String,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub date: Option<String>,
    pub original_date: Option<String>,
    pub genre: Option<String>,
    pub label: Option<String>,
    pub disc: Option<String>,
    pub mb_release_id: Option<String>,
    pub mb_release_group_id: Option<String>,
    pub mb_albumartist_id: Option<String>,
}

/// A tag value that is actually there: trimmed, and `None` if it was absent
/// or blank. Shared by every derivation that reads `SongTags`, in this crate
/// and in `album-tool`'s grouping.
pub fn present_tag(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|s| !s.is_empty())
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
