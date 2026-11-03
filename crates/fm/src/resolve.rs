//! Which automation a person meant: its full key, its name (optionally `user/` or `repository/`),
//! or a unique prefix of its name. Never a guess.

use fm_model::{Scope, Status};

#[derive(Debug, PartialEq, Eq)]
pub enum Match<'a> {
    One(&'a Status),
    Ambiguous(Vec<&'a Status>),
    None,
}

pub fn resolve<'a>(statuses: &'a [Status], arg: &str) -> Match<'a> {
    if let Some(s) = statuses.iter().find(|s| s.key == arg) {
        return Match::One(s);
    }
    let (scope, wanted) = if let Some(rest) = arg.strip_prefix("user/") {
        (Some(Scope::User), rest)
    } else if let Some(rest) = arg.strip_prefix("repository/") {
        (Some(Scope::Repository), rest)
    } else if let Some(rest) = arg.strip_prefix("project/") {
        // pa's old prefix.
        (Some(Scope::Repository), rest)
    } else {
        (None, arg)
    };
    let in_scope = |s: &&Status| scope.is_none_or(|sc| s.scope == sc);
    let pick = |matches: Vec<&'a Status>| match matches.len() {
        0 => Match::None,
        1 => Match::One(matches[0]),
        _ => Match::Ambiguous(matches),
    };
    let exact: Vec<&Status> = statuses
        .iter()
        .filter(in_scope)
        .filter(|s| s.name == wanted)
        .collect();
    if !exact.is_empty() {
        return pick(exact);
    }
    if wanted.is_empty() {
        return Match::None;
    }
    pick(
        statuses
            .iter()
            .filter(in_scope)
            .filter(|s| s.name.starts_with(wanted))
            .collect(),
    )
}

/// `ambiguous "x": <key>, <key> — use the full key`
pub fn ambiguous_line(arg: &str, matches: &[&Status]) -> String {
    let keys: Vec<&str> = matches.iter().map(|s| s.key.as_str()).collect();
    format!(
        "ambiguous {}: {} — use the full key",
        quoted(arg),
        keys.join(", ")
    )
}

pub fn not_found_line(arg: &str) -> String {
    format!("no automation named {} — fm list shows them", quoted(arg))
}

fn quoted(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fm_model::AutomationState;
    use std::path::PathBuf;

    fn st(scope: Scope, name: &str) -> Status {
        let s = match scope {
            Scope::User => "user",
            Scope::Repository => "repository",
        };
        Status {
            key: format!("{s}:/r:{name}"),
            name: name.into(),
            scope,
            dir: PathBuf::from(name),
            state: AutomationState::Off,
            reason: None,
            problems: vec![],
            automation: None,
            agent: None,
            effective_timeout_ms: 0,
            next_fire: None,
            next_check: None,
            watch: None,
            watch_paused_at: None,
            running: None,
            flow: false,
            last: None,
            also_armed_in_pa: false,
        }
    }

    #[test]
    fn key_name_prefix_and_scope() {
        let all = vec![
            st(Scope::User, "daily"),
            st(Scope::User, "daily-notes"),
            st(Scope::Repository, "weekly"),
            st(Scope::User, "weekly"),
        ];
        assert!(matches!(resolve(&all, "daily"), Match::One(s) if s.name == "daily"));
        assert!(matches!(resolve(&all, "daily-n"), Match::One(s) if s.name == "daily-notes"));
        assert!(matches!(resolve(&all, "dai"), Match::Ambiguous(v) if v.len() == 2));
        assert!(matches!(resolve(&all, "weekly"), Match::Ambiguous(v) if v.len() == 2));
        assert!(
            matches!(resolve(&all, "repository/weekly"), Match::One(s) if s.scope == Scope::Repository)
        );
        assert!(matches!(resolve(&all, "user:/r:weekly"), Match::One(s) if s.scope == Scope::User));
        assert_eq!(resolve(&all, "nope"), Match::None);
        assert_eq!(resolve(&all, "user/"), Match::None);
    }

    #[test]
    fn lines_are_one_line() {
        let all = [st(Scope::User, "a"), st(Scope::Repository, "a")];
        let refs: Vec<&Status> = all.iter().collect();
        let l = ambiguous_line("a", &refs);
        assert_eq!(
            l,
            "ambiguous \"a\": user:/r:a, repository:/r:a — use the full key"
        );
        assert!(not_found_line("x").contains(" — "));
    }
}
