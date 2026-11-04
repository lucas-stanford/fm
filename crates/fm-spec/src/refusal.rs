//! The one-line refusal every check produces: `<field path>: <what is wrong> — <what fixes it>`.

use std::fmt;

/// Why a file, or a machine check, refuses an automation.
///
/// `Display` gives the one line. `problems` is non-empty only when one check found several things
/// wrong (strict flow): each is one problem without the field prefix, and `message` joins them with
/// `; `.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The field path, e.g. `on.schedule.cron` or `hooks.pre[0].script`.
    pub field: String,
    /// Everything after `<field>: `.
    pub message: String,
    pub problems: Vec<String>,
}

impl Refusal {
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Refusal {
            field: field.into(),
            message: message.into(),
            problems: Vec::new(),
        }
    }

    /// One refusal for several problems of one field, joined with `; `.
    pub fn with_problems(field: impl Into<String>, problems: Vec<String>) -> Self {
        Refusal {
            field: field.into(),
            message: problems.join("; "),
            problems,
        }
    }

    /// Split a line that is already `<field>: <message>`, e.g. one an agent adapter produced.
    pub fn from_line(line: &str) -> Self {
        match line.split_once(": ") {
            Some((field, message)) => Refusal::new(field, message),
            None => Refusal::new("", line),
        }
    }

    /// The one line, `<field>: <message>`.
    pub fn line(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.field.is_empty() {
            f.write_str(&self.message)
        } else {
            write!(f, "{}: {}", self.field, self.message)
        }
    }
}

impl std::error::Error for Refusal {}

/// A string as `JSON.stringify` writes it, which is how every message quotes a value.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_line_shape() {
        let r = Refusal::new("name", "required — give one");
        assert_eq!(r.line(), "name: required — give one");
        assert_eq!(Refusal::from_line(&r.line()), r);
    }

    #[test]
    fn problems_are_joined() {
        let r = Refusal::with_problems("flow", vec!["line 3: a".into(), "b".into()]);
        assert_eq!(r.to_string(), "flow: line 3: a; b");
        assert_eq!(r.problems.len(), 2);
    }

    #[test]
    fn quotes_like_json() {
        assert_eq!(quote("Daily_Report"), "\"Daily_Report\"");
        assert_eq!(quote("a\"b\\c\n"), "\"a\\\"b\\\\c\\n\"");
        assert_eq!(quote("≤64"), "\"≤64\"");
    }
}
