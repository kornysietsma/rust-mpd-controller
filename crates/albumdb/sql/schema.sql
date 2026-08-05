CREATE TABLE IF NOT EXISTS albums (
    album_id            INTEGER PRIMARY KEY,
    path                TEXT    NOT NULL UNIQUE,   -- 'Albums/E/EELS/EELS TIME!'

    -- identity / presence
    status              TEXT    NOT NULL DEFAULT 'present',  -- present | missing
    fingerprint         TEXT,                       -- for rename detection
    first_seen          TEXT    NOT NULL,
    last_seen           TEXT    NOT NULL,

    -- descriptive, from tags or path
    title               TEXT,
    artist              TEXT,                       -- AlbumArtist
    title_source        TEXT,                       -- tag | path | manual
    date                TEXT,                       -- release date of this edition
    original_date       TEXT,                       -- OriginalDate; '0000' -> NULL
    label               TEXT,
    track_count         INTEGER NOT NULL,
    disc_count          INTEGER NOT NULL DEFAULT 1,

    -- MusicBrainz identifiers, from file tags
    mb_release_id       TEXT,
    mb_release_group_id TEXT,
    mb_albumartist_id   TEXT,

    -- derived every scan, never hand-edited
    tag_state           TEXT    NOT NULL,

    -- written only by `sync`
    mb_sync_state       TEXT    NOT NULL DEFAULT 'pending', -- pending|ok|failed|skipped
    mb_synced_at        TEXT,
    mb_sync_error       TEXT,
    mb_original_count   INTEGER,        -- track count of earliest release in group
    primary_genre       TEXT,           -- cached top-voted MB genre

    -- reserved for the deferred `verify` command
    mb_verify_state     TEXT    NOT NULL DEFAULT 'unverified', -- unverified|ok|drifted
    mb_verified_at      TEXT,
    mb_verify_notes     TEXT,

    -- user-editable; never machine-written
    rating              INTEGER,
    needs_relisten      INTEGER NOT NULL DEFAULT 0,
    rated_at            TEXT,
    is_classical        INTEGER NOT NULL DEFAULT 0,
    is_spoken           INTEGER NOT NULL DEFAULT 0,
    is_soundtrack       INTEGER NOT NULL DEFAULT 0,
    original_tracks     TEXT,           -- '1-10' | '1-12,14' | NULL = all
    original_tracks_src TEXT,           -- manual | mb
    match_policy        TEXT    NOT NULL DEFAULT 'auto',  -- auto | never | recheck
    notes               TEXT,

    CHECK (rating IS NULL OR rating BETWEEN 0 AND 5),
    CHECK (status IN ('present','missing')),
    CHECK (match_policy IN ('auto','never','recheck')),
    CHECK (tag_state IN ('clean','mixed_mbid','partial_mbid',
                         'no_mbid_consistent','no_mbid_varied','no_mbid_incomplete'))
);

CREATE INDEX IF NOT EXISTS idx_albums_sync    ON albums(mb_sync_state) WHERE status = 'present';
CREATE INDEX IF NOT EXISTS idx_albums_rg      ON albums(mb_release_group_id);
CREATE INDEX IF NOT EXISTS idx_albums_rating  ON albums(rating);

CREATE TABLE IF NOT EXISTS album_genres (
    album_id   INTEGER NOT NULL REFERENCES albums(album_id) ON DELETE CASCADE,
    genre      TEXT    NOT NULL,      -- 'dark cabaret'
    votes      INTEGER NOT NULL DEFAULT 0,
    source     TEXT    NOT NULL,      -- 'mb' | 'tag'
    PRIMARY KEY (album_id, genre, source)
);

CREATE INDEX IF NOT EXISTS idx_genres_genre ON album_genres(genre);
