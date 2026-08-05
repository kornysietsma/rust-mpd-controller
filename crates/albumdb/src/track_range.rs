use std::fmt;
use std::str::FromStr;

/// The `original_tracks` column: a set of 1-based, inclusive track spans,
/// e.g. `1-10` or `1-12,14`. `None` at the call site means "all tracks" —
/// that case is represented by the column being `NULL`, not by a value of
/// this type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackRange(Vec<(u32, u32)>);

impl fmt::Display for TrackRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self
            .0
            .iter()
            .map(|&(start, end)| {
                if start == end {
                    start.to_string()
                } else {
                    format!("{start}-{end}")
                }
            })
            .collect();
        write!(f, "{}", parts.join(","))
    }
}

impl FromStr for TrackRange {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.trim().is_empty() {
            return Err("empty track range".to_string());
        }

        let mut spans = Vec::new();
        for part in s.split(',') {
            let part = part.trim();
            let (start, end) = match part.split_once('-') {
                Some((start_s, end_s)) => {
                    let start: u32 = start_s
                        .parse()
                        .map_err(|_| format!("invalid track range segment: {part:?}"))?;
                    let end: u32 = end_s
                        .parse()
                        .map_err(|_| format!("invalid track range segment: {part:?}"))?;
                    (start, end)
                }
                None => {
                    let n: u32 = part
                        .parse()
                        .map_err(|_| format!("invalid track number: {part:?}"))?;
                    (n, n)
                }
            };
            if start < 1 {
                return Err(format!("track numbers start at 1: {part:?}"));
            }
            if start > end {
                return Err(format!("range start after end: {part:?}"));
            }
            spans.push((start, end));
        }
        Ok(TrackRange(spans))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parses_and_formats_a_single_range() {
        let range: TrackRange = "1-10".parse().unwrap();
        assert_eq!(range.to_string(), "1-10");
    }

    #[test]
    fn parses_and_formats_a_range_plus_singleton() {
        let range: TrackRange = "1-12,14".parse().unwrap();
        assert_eq!(range.to_string(), "1-12,14");
    }

    #[test]
    fn round_trips_parse_of_format() {
        for s in ["1-10", "1-12,14", "3", "1-2,4-6,9"] {
            let range: TrackRange = s.parse().unwrap();
            let reparsed: TrackRange = range.to_string().parse().unwrap();
            assert_eq!(range, reparsed);
        }
    }

    #[test]
    fn rejects_zero_start() {
        assert!("0-5".parse::<TrackRange>().is_err());
    }

    #[test]
    fn rejects_inverted_range() {
        assert!("5-1".parse::<TrackRange>().is_err());
    }

    #[test]
    fn rejects_non_numeric() {
        assert!("abc".parse::<TrackRange>().is_err());
    }

    #[test]
    fn rejects_empty() {
        assert!("".parse::<TrackRange>().is_err());
    }
}
