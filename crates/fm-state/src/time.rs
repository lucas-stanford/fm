//! Timestamps: ISO 8601 in UTC with milliseconds, as pi-automations writes them.

use chrono::{DateTime, SecondsFormat, Utc};

/// `2026-09-17T12:00:00.000Z`.
pub fn iso(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Any RFC 3339 timestamp, in UTC. `None` for anything else.
pub fn parse_iso(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// Milliseconds from `a` to `b`, negative when `b` is earlier.
pub fn ms_between(a: DateTime<Utc>, b: DateTime<Utc>) -> i64 {
    (b - a).num_milliseconds()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let t = parse_iso("2026-09-17T14:00:00+02:00").unwrap();
        assert_eq!(iso(t), "2026-09-17T12:00:00.000Z");
        assert_eq!(parse_iso(&iso(t)), Some(t));
        assert!(parse_iso("yesterday").is_none());
        assert_eq!(ms_between(t, t + chrono::Duration::seconds(2)), 2000);
    }
}
