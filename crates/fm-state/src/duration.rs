//! Durations: `s`/`m`/`h` in `AUTOMATION.md`; days too in `config.toml`.

/// Parse a duration to milliseconds. `None` means "not a duration"; zero, leading zeros, spaces
/// and compound forms are not durations.
pub fn parse_duration(s: &str, allow_days: bool) -> Option<u64> {
    let unit = s.chars().last()?;
    let digits = &s[..s.len() - unit.len_utf8()];
    let mult: u64 = match unit {
        's' => 1_000,
        'm' => 60_000,
        'h' => 3_600_000,
        'd' if allow_days => 86_400_000,
        _ => return None,
    };
    if digits.is_empty() || digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u64>().ok()?.checked_mul(mult)
}

/// Milliseconds the way the listing shows them: `45s`, `9m12s`, `1h`. A display format, not the
/// inverse of [`parse_duration`].
pub fn format_duration(ms: u64) -> String {
    let total = (ms + 500) / 1000;
    if total == 0 {
        return "0s".into();
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

    #[test]
    fn accepts_the_automation_md_grammar() {
        assert_eq!(parse_duration("90s", false), Some(90_000));
        assert_eq!(parse_duration("10m", false), Some(600_000));
        assert_eq!(parse_duration("1h", false), Some(3_600_000));
    }

    #[test]
    fn refuses_zero_whitespace_compounds_and_other_units() {
        for s in [
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
            "1µ",
        ] {
            assert_eq!(parse_duration(s, true), None, "{s}");
        }
    }

    #[test]
    fn allows_days_only_when_asked() {
        assert_eq!(parse_duration("7d", false), None);
        assert_eq!(parse_duration("7d", true), Some(604_800_000));
        assert_eq!(parse_duration("30d", true), Some(2_592_000_000));
    }

    #[test]
    fn formats_like_the_listing() {
        assert_eq!(format_duration(45_000), "45s");
        assert_eq!(format_duration(552_000), "9m12s");
        assert_eq!(format_duration(3_600_000), "1h");
        assert_eq!(format_duration(3_660_000), "1h1m");
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(90_000), "1m30s");
        assert_eq!(parse_duration(&format_duration(90_000), false), None);
        for s in ["1s", "59s", "1m", "10m", "1h", "4h"] {
            let ms = parse_duration(s, false).unwrap();
            assert_eq!(parse_duration(&format_duration(ms), false), Some(ms));
        }
    }
}
