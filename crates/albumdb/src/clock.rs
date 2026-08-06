use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Injected time source (plan §1.1) — every write of `first_seen`,
/// `last_seen`, `rated_at` or `mb_synced_at` takes an `OffsetDateTime`
/// explicitly rather than calling `OffsetDateTime::now_utc()` inline, so
/// callers (including rate-limiting logic in `sync`) are deterministic under
/// test.
pub trait Clock {
    fn now(&self) -> OffsetDateTime;
}

/// The real clock, used outside of tests.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

/// A clock that always returns the same instant. Public (not `cfg(test)`)
/// because downstream crates — `album-tool`'s own tests — need it too, and a
/// `cfg(test)` item in this crate isn't visible to a dependent crate's test
/// build.
pub struct FixedClock(pub OffsetDateTime);

impl Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        self.0
    }
}

/// Format for storage in a `TEXT` column: RFC 3339, e.g.
/// `2026-08-05T12:34:56Z`.
pub fn format_timestamp(instant: OffsetDateTime) -> String {
    instant
        .format(&Rfc3339)
        .expect("RFC 3339 formatting is infallible for a valid OffsetDateTime")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use time::macros::datetime;

    #[test]
    fn fixed_clock_always_returns_the_same_instant() {
        let instant = datetime!(2026-08-05 12:34:56 UTC);
        let clock = FixedClock(instant);
        assert_eq!(clock.now(), instant);
        assert_eq!(clock.now(), instant);
    }

    #[test]
    fn format_timestamp_produces_rfc3339() {
        let instant = datetime!(2026-08-05 12:34:56 UTC);
        assert_eq!(format_timestamp(instant), "2026-08-05T12:34:56Z");
    }
}
