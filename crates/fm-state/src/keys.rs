//! Automation keys (`<scope>:<root path>:<name>`) and path normalisation.

use std::path::{Component, Path, PathBuf};

use fm_model::Scope;

/// An automation key, parsed. The root is between the first and the last colon, so a Windows drive
/// colon stays in the root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationKey {
    pub scope: Scope,
    pub root: PathBuf,
    pub name: String,
}

impl AutomationKey {
    /// `None` for anything that is not `<scope>:<root>:<name>` with a known scope. `project` is the
    /// older spelling of `repository` (pi-automations wrote it once) and reads as one.
    pub fn parse(key: &str) -> Option<AutomationKey> {
        let (scope, rest) = key.split_once(':')?;
        let (root, name) = rest.rsplit_once(':')?;
        let scope = match scope {
            "user" => Scope::User,
            "repository" | "project" => Scope::Repository,
            _ => return None,
        };
        if root.is_empty() || name.is_empty() {
            return None;
        }
        Some(AutomationKey {
            scope,
            root: PathBuf::from(root),
            name: name.to_string(),
        })
    }

    /// The same automation: same scope, same root folder (however spelled), same name.
    pub fn same_as(&self, other: &AutomationKey) -> bool {
        self.scope == other.scope && self.name == other.name && same_path(&self.root, &other.root)
    }
}

/// The name part of a key: everything after the last colon.
pub fn name_of(key: &str) -> &str {
    key.rsplit_once(':').map_or(key, |(_, n)| n)
}

/// The root part of a key: between the first and the last colon.
pub fn root_of(key: &str) -> &str {
    match (key.find(':'), key.rfind(':')) {
        (Some(first), Some(last)) if last > first => &key[first + 1..last],
        _ => "",
    }
}

/// The scope of a key; anything that is not `user:` is repository scope.
pub fn scope_of(key: &str) -> Scope {
    if key.starts_with("user:") {
        Scope::User
    } else {
        Scope::Repository
    }
}

/// Absolute, with `.` and `..` resolved lexically and no trailing separator. Never touches the
/// filesystem beyond what `std::path::absolute` needs (the working directory).
pub fn normalize_path(p: &Path) -> PathBuf {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::Prefix(_) | Component::RootDir => out.push(c.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                }
            }
            Component::Normal(n) => out.push(n),
        }
    }
    out
}

/// Two spellings of one folder. Case-insensitive on Windows and macOS, whose usual filesystems are.
pub fn same_path(a: &Path, b: &Path) -> bool {
    let (a, b) = (normalize_path(a), normalize_path(b));
    if cfg!(any(windows, target_os = "macos")) {
        let fold = |p: &Path| p.to_string_lossy().replace('/', "\\").to_lowercase();
        fold(&a) == fold(&b)
    } else {
        a == b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_keys_with_drive_colons() {
        let k = AutomationKey::parse("user:D:\\Users\\me\\.agents\\automations:daily").unwrap();
        assert_eq!(k.scope, Scope::User);
        assert_eq!(k.root, PathBuf::from("D:\\Users\\me\\.agents\\automations"));
        assert_eq!(k.name, "daily");
        assert_eq!(
            root_of("user:D:\\x:daily"),
            "D:\\x",
            "root is between the first and last colon"
        );
        assert_eq!(name_of("repository:/r:x"), "x");
    }

    #[test]
    fn reads_project_as_repository_and_refuses_junk() {
        let k = AutomationKey::parse("project:/srv/r/.agents/automations:x").unwrap();
        assert_eq!(k.scope, Scope::Repository);
        assert!(AutomationKey::parse("team:/r:x").is_none());
        assert!(AutomationKey::parse("user:x").is_none());
        assert!(AutomationKey::parse("user::x").is_none());
    }

    #[test]
    fn normalizes_two_spellings_to_one() {
        let base = std::env::temp_dir();
        let a = base.join("fm-x").join("..").join("fm-y").join(".");
        assert_eq!(normalize_path(&a), normalize_path(&base.join("fm-y")));
        assert!(same_path(&base.join("fm-y"), &a));
        assert!(!same_path(&base.join("fm-y"), &base.join("fm-z")));
    }

    #[test]
    fn matches_keys_by_scope_root_and_name() {
        let root = std::env::temp_dir()
            .join("fm-keys")
            .join(".agents")
            .join("automations");
        let a = AutomationKey::parse(&format!("repository:{}:x", root.display())).unwrap();
        let b = AutomationKey::parse(&format!("project:{}:x", root.join(".").display())).unwrap();
        let c = AutomationKey::parse(&format!("user:{}:x", root.display())).unwrap();
        let d = AutomationKey::parse(&format!("repository:{}:y", root.display())).unwrap();
        assert!(a.same_as(&b));
        assert!(!a.same_as(&c));
        assert!(!a.same_as(&d));
    }
}
