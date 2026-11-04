//! The one error type of this crate. Every message is one line, `<what> — <what fixes it>`.

use std::io;
use std::path::{Path, PathBuf};

pub type Result<T, E = StateError> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    /// A file in the state folder could not be read or written.
    #[error("{}: {source} — check that the state folder is writable", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// A short read-change-write lock stayed busy.
    #[error("{}: still locked after {waited_ms} ms — another host process is using it; try again", path.display())]
    LockTimeout { path: PathBuf, waited_ms: u64 },
    /// `config.toml` refused, already one line starting `config.toml:`.
    #[error("{0}")]
    Config(String),
    /// The state folder cannot be found.
    #[error("state folder: {0}")]
    Folder(String),
}

impl StateError {
    pub(crate) fn io(path: &Path, source: io::Error) -> Self {
        StateError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

/// Attach the path to an I/O error.
pub(crate) trait IoContext<T> {
    fn at(self, path: &Path) -> Result<T>;
}

impl<T> IoContext<T> for io::Result<T> {
    fn at(self, path: &Path) -> Result<T> {
        self.map_err(|e| StateError::io(path, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_one_line_with_a_fix() {
        let errors = [
            StateError::io(Path::new("x.json"), io::Error::other("boom")),
            StateError::LockTimeout {
                path: PathBuf::from("trust.json.lock"),
                waited_ms: 10,
            },
            StateError::Folder("no home — set FM_HOME".into()),
        ];
        for e in errors {
            let s = e.to_string();
            assert!(!s.contains('\n'), "{s}");
            assert!(s.contains(" — "), "{s}");
        }
    }
}
