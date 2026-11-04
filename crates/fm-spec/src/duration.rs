//! Durations: a positive whole number and `s`, `m` or `h` in `AUTOMATION.md`; `d` too in config.

use std::time::Duration;

fn parse_with(s: &str, units: &[char]) -> Option<Duration> {
    let unit = s.chars().last()?;
    if !units.contains(&unit) {
        return None;
    }
    let digits = &s[..s.len() - unit.len_utf8()];
    let mut chars = digits.chars();
    match chars.next() {
        Some('1'..='9') => {}
        _ => return None,
    }
    if !chars.all(|c| c.is_ascii_digit()) {
        return None;
    }
    // Too many digits for a u64 is still a duration, just a very long one.
    let n: u64 = digits.parse().unwrap_or(u64::MAX);
    let secs = match unit {
        's' => n,
        'm' => n.saturating_mul(60),
        'h' => n.saturating_mul(3_600),
        'd' => n.saturating_mul(86_400),
        _ => return None,
    };
    Some(Duration::from_secs(secs))
}

/// `90s`, `10m`, `1h`. `None` for anything else, compound forms and zero included.
pub fn parse_duration(s: &str) -> Option<Duration> {
    parse_with(s, &['s', 'm', 'h'])
}

/// The same grammar plus `d`, for host settings such as consent expiry.
pub fn parse_duration_with_days(s: &str) -> Option<Duration> {
    parse_with(s, &['s', 'm', 'h', 'd'])
}

/// How listings show a duration: `45s`, `9m12s`, `1h`. A display format, not always re-parseable.
pub fn format_duration(d: Duration) -> String {
    let total = (d.as_millis() + 500) / 1000;
    if total == 0 {
        return "0s".to_string();
    }
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    let mut out = String::new();
    if h > 0 {
        out.push_str(&format!("{h}h"));
    }
    if m > 0 {
        out.push_str(&format!("{m}m"));
    }
    if s > 0 {
        out.push_str(&format!("{s}s"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(s: &str) -> Option<u128> {
        parse_duration(s).map(|d| d.as_millis())
    }

    #[test]
    fn accepts_the_automation_grammar() {
        assert_eq!(ms("90s"), Some(90_000));
        assert_eq!(ms("10m"), Some(600_000));
        assert_eq!(ms("1h"), Some(3_600_000));
    }

    #[test]
    fn refuses_zero_whitespace_compound_and_other_units() {
        for bad in [
            "0s",
            "0m",
            "10 m",
            "1h30m",
            " 10m",
            "10m ",
            "10ms",
            "2w",
            "ten minutes",
            "01m",
            "-1m",
            "",
            "m",
            "7d",
        ] {
            assert_eq!(parse_duration(bad), None, "{bad}");
        }
    }

    #[test]
    fn allows_days_only_when_asked() {
        assert_eq!(
            parse_duration_with_days("7d"),
            Some(Duration::from_secs(604_800))
        );
        assert_eq!(
            parse_duration_with_days("30d").map(|d| d.as_millis()),
            Some(2_592_000_000)
        );
    }

    #[test]
    fn saturates_rather_than_overflowing() {
        assert!(parse_duration("99999999999999999999999h").is_some());
    }

    #[test]
    fn formats_for_listings() {
        let f = |ms: u64| format_duration(Duration::from_millis(ms));
        assert_eq!(f(45_000), "45s");
        assert_eq!(f(552_000), "9m12s");
        assert_eq!(f(3_600_000), "1h");
        assert_eq!(f(600_000), "10m");
        assert_eq!(f(3_660_000), "1h1m");
        assert_eq!(f(14_400_000), "4h");
        assert_eq!(f(90_000), "1m30s");
        assert_eq!(f(0), "0s");
        assert_eq!(parse_duration(&f(90_000)), None);
        for s in ["1s", "59s", "1m", "10m", "1h", "4h"] {
            let d = parse_duration(s).unwrap();
            assert_eq!(parse_duration(&format_duration(d)), Some(d));
        }
    }
}
