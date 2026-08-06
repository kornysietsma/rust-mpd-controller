//! String enums for `albums` columns, matching the schema exactly (§4.2).
//!
//! `status`, `match_policy` and `tag_state` are also enforced by a SQL
//! `CHECK` constraint; `title_source`, `mb_sync_state` and `mb_verify_state`
//! are documented in the schema by comment only and enforced here instead.
//! Either way, `as_str`/`FromStr` are the single source of truth for the
//! allowed values.

use std::str::FromStr;

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $str:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $str),+
                }
            }
        }

        impl FromStr for $name {
            type Err = String;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($str => Ok($name::$variant),)+
                    other => Err(format!(concat!("invalid ", stringify!($name), ": {:?}"), other)),
                }
            }
        }
    };
}

string_enum!(AlbumStatus {
    Present => "present",
    Missing => "missing",
});

string_enum!(GenreSource {
    Tag => "tag",
    Mb => "mb",
});

string_enum!(TagState {
    Clean => "clean",
    MixedMbid => "mixed_mbid",
    PartialMbid => "partial_mbid",
    NoMbidConsistent => "no_mbid_consistent",
    NoMbidVaried => "no_mbid_varied",
    NoMbidIncomplete => "no_mbid_incomplete",
});

string_enum!(TitleSource {
    Tag => "tag",
    Path => "path",
    Manual => "manual",
});

string_enum!(MatchPolicy {
    Auto => "auto",
    Never => "never",
    Recheck => "recheck",
});

string_enum!(SyncState {
    Pending => "pending",
    Ok => "ok",
    Failed => "failed",
    Skipped => "skipped",
});

string_enum!(VerifyState {
    Unverified => "unverified",
    Ok => "ok",
    Drifted => "drifted",
});

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn album_status_round_trips_every_variant() {
        for s in ["present", "missing"] {
            assert_eq!(AlbumStatus::from_str(s).unwrap().as_str(), s);
        }
    }

    #[test]
    fn genre_source_round_trips_every_variant() {
        for s in ["tag", "mb"] {
            assert_eq!(GenreSource::from_str(s).unwrap().as_str(), s);
        }
    }

    #[test]
    fn tag_state_round_trips_every_variant() {
        for s in [
            "clean",
            "mixed_mbid",
            "partial_mbid",
            "no_mbid_consistent",
            "no_mbid_varied",
            "no_mbid_incomplete",
        ] {
            assert_eq!(TagState::from_str(s).unwrap().as_str(), s);
        }
    }

    #[test]
    fn title_source_round_trips_every_variant() {
        for s in ["tag", "path", "manual"] {
            assert_eq!(TitleSource::from_str(s).unwrap().as_str(), s);
        }
    }

    #[test]
    fn match_policy_round_trips_every_variant() {
        for s in ["auto", "never", "recheck"] {
            assert_eq!(MatchPolicy::from_str(s).unwrap().as_str(), s);
        }
    }

    #[test]
    fn sync_state_round_trips_every_variant() {
        for s in ["pending", "ok", "failed", "skipped"] {
            assert_eq!(SyncState::from_str(s).unwrap().as_str(), s);
        }
    }

    #[test]
    fn verify_state_round_trips_every_variant() {
        for s in ["unverified", "ok", "drifted"] {
            assert_eq!(VerifyState::from_str(s).unwrap().as_str(), s);
        }
    }

    #[test]
    fn from_str_rejects_unknown_value() {
        assert!(TagState::from_str("nonsense").is_err());
        assert!(TitleSource::from_str("").is_err());
        assert!(MatchPolicy::from_str("maybe").is_err());
        assert!(SyncState::from_str("done").is_err());
        assert!(VerifyState::from_str("verified").is_err());
    }
}
