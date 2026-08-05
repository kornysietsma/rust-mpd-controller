/// Derive `(title, artist)` from an album directory path when tags aren't
/// trustworthy enough to use, per spec §5.3.
///
/// | Pattern | title | artist |
/// |---|---|---|
/// | `Albums/<letter>/<artist>/<album>` | `<album>` | `<artist>` |
/// | `Compilations/<album>` | `<album>` | `Various Artists` |
/// | `Unnmatched/<artist>/<album>` | `<album>` | `<artist>` |
/// | `Unnmatched/<album>` | `<album>` | `None` |
///
/// A path matching none of these is an error, not a best-effort guess: the
/// library only ever has these three top-level directories at these depths,
/// so an unmatched path means either the data is wrong or a new case has
/// appeared that this function doesn't know about yet — both worth seeing,
/// neither worth silently papering over.
pub fn parse_album_path(path: &str) -> Result<(Option<String>, Option<String>), String> {
    let parts: Vec<&str> = path.split('/').collect();
    match parts.as_slice() {
        ["Albums", _letter, artist, album] => {
            Ok((Some((*album).to_string()), Some((*artist).to_string())))
        }
        ["Compilations", album] => Ok((
            Some((*album).to_string()),
            Some("Various Artists".to_string()),
        )),
        ["Unnmatched", artist, album] => {
            Ok((Some((*album).to_string()), Some((*artist).to_string())))
        }
        ["Unnmatched", album] => Ok((Some((*album).to_string()), None)),
        _ => Err(format!("album path matches no known pattern: {path:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn albums_pattern() {
        assert_eq!(
            parse_album_path("Albums/E/EELS/EELS TIME!"),
            Ok((Some("EELS TIME!".to_string()), Some("EELS".to_string())))
        );
    }

    #[test]
    fn compilations_pattern() {
        assert_eq!(
            parse_album_path("Compilations/Sound Environment"),
            Ok((
                Some("Sound Environment".to_string()),
                Some("Various Artists".to_string())
            ))
        );
    }

    #[test]
    fn unnmatched_with_artist_pattern() {
        assert_eq!(
            parse_album_path("Unnmatched/Madness/Absolutely"),
            Ok((Some("Absolutely".to_string()), Some("Madness".to_string())))
        );
    }

    #[test]
    fn unnmatched_without_artist_pattern() {
        assert_eq!(
            parse_album_path("Unnmatched/Some Compilation"),
            Ok((Some("Some Compilation".to_string()), None))
        );
    }

    #[test]
    fn apostrophe_in_path_segment_is_preserved() {
        assert_eq!(
            parse_album_path(
                "Compilations/A Wake-Up Call for the '90s_ Epic_Associated Hard Rock Sampler"
            ),
            Ok((
                Some("A Wake-Up Call for the '90s_ Epic_Associated Hard Rock Sampler".to_string()),
                Some("Various Artists".to_string())
            ))
        );
    }

    #[test]
    fn unmatched_pattern_is_an_error_not_a_guess() {
        assert!(parse_album_path("Albums/EELS TIME!").is_err());
        assert!(parse_album_path("").is_err());
        assert!(parse_album_path("SomethingElse/Deep/Nested/Path/Here").is_err());
    }
}
