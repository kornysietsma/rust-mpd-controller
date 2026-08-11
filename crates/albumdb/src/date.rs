/// Normalise a `Date`/`OriginalDate` tag value: blank or the junk year
/// `'0000'` become `None`, everything else passes through unchanged.
pub fn normalise_date(date: Option<&str>) -> Option<String> {
    let trimmed = date?.trim();
    if trimmed.is_empty() || trimmed == "0000" {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn junk_year_becomes_none() {
        assert_eq!(normalise_date(Some("0000")), None);
    }

    #[test]
    fn blank_becomes_none() {
        assert_eq!(normalise_date(Some("")), None);
        assert_eq!(normalise_date(Some("   ")), None);
        assert_eq!(normalise_date(None), None);
    }

    #[test]
    fn real_date_passes_through() {
        assert_eq!(normalise_date(Some("1975")), Some("1975".to_string()));
        assert_eq!(
            normalise_date(Some("1975-06-20")),
            Some("1975-06-20".to_string())
        );
    }
}
