//! The live view of a run: [`LiveEvent`]s as JSON lines in `locks/<slug>.live`, written by the run
//! itself while it goes and removed when it ends. A view nobody can write is a view nobody sees,
//! never a failed run: nothing here returns an error or panics.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use fm_model::LiveEvent;

/// How often streaming text is written: often enough to read as live, rarely enough to be free.
pub const PARTIAL_MS: u64 = 1000;
/// The most of a reply or reasoning still arriving that is kept (its end).
pub const PARTIAL_MAX: usize = 2000;
/// The most of a finished message or answer that is kept (its start).
pub const MESSAGE_MAX: usize = 8000;
/// A tool summary is one line of at most this many characters.
pub const SUMMARY_MAX: usize = 120;
/// An error is one line of at most this many characters.
pub const ERROR_MAX: usize = 500;

/// The last `max` characters of `s`.
pub(crate) fn tail_chars(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    s.chars().skip(n - max).collect()
}

/// The first `max` characters of `s`.
pub(crate) fn head_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// `s` on one line (whitespace runs become one space), at most `max` characters.
pub(crate) fn one_line(s: &str, max: usize) -> String {
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() <= max {
        return joined;
    }
    let mut cut: String = joined.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// Milliseconds since the Unix epoch, for event timestamps.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Appends a run's live events to its live file: bounded, with streaming text throttled.
#[derive(Debug)]
pub struct LiveWriter {
    file: PathBuf,
    out: Option<File>,
    last_partial_at: Option<u64>,
    last_partial: String,
}

impl LiveWriter {
    /// Start the live file afresh: whatever a previous run left is dropped.
    pub fn create(file: impl Into<PathBuf>) -> LiveWriter {
        let file = file.into();
        if let Some(dir) = file.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let mut o = OpenOptions::new();
        o.create(true).write(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let out = o.open(&file).ok();
        LiveWriter {
            file,
            out,
            last_partial_at: None,
            last_partial: String::new(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.file
    }

    /// Write one event, bounded. Text still arriving is skipped when it is blank, unchanged, or
    /// less than [`PARTIAL_MS`] after the last; a finished message lets the next one through.
    pub fn write(&mut self, event: LiveEvent) {
        let event = match event {
            LiveEvent::Partial { text, ts } => {
                if !self.let_through(&text, ts) {
                    return;
                }
                LiveEvent::Partial {
                    text: tail_chars(&text, PARTIAL_MAX),
                    ts,
                }
            }
            LiveEvent::Thinking { text, ts } => {
                if !self.let_through(&text, ts) {
                    return;
                }
                LiveEvent::Thinking {
                    text: tail_chars(&text, PARTIAL_MAX),
                    ts,
                }
            }
            LiveEvent::Message { text, ts } => {
                self.last_partial_at = None;
                self.last_partial.clear();
                LiveEvent::Message {
                    text: head_chars(&text, MESSAGE_MAX),
                    ts,
                }
            }
            LiveEvent::Final { text, ts } => LiveEvent::Final {
                text: head_chars(&text, MESSAGE_MAX),
                ts,
            },
            LiveEvent::Tool { name, summary, ts } => LiveEvent::Tool {
                name: one_line(&name, SUMMARY_MAX),
                summary: one_line(&summary, SUMMARY_MAX),
                ts,
            },
            LiveEvent::Error { message, ts } => LiveEvent::Error {
                message: one_line(&message, ERROR_MAX),
                ts,
            },
            other => other,
        };
        let Some(out) = self.out.as_mut() else {
            return;
        };
        if let Ok(mut line) = serde_json::to_string(&event) {
            line.push('\n');
            if out.write_all(line.as_bytes()).is_err() {
                // Stop trying: a view that cannot be written is a view nobody sees.
                self.out = None;
            }
        }
    }

    fn let_through(&mut self, text: &str, ts: u64) -> bool {
        if text.trim().is_empty() || text == self.last_partial {
            return false;
        }
        if self
            .last_partial_at
            .is_some_and(|at| ts.saturating_sub(at) < PARTIAL_MS)
        {
            return false;
        }
        self.last_partial_at = Some(ts);
        self.last_partial = text.to_string();
        true
    }

    /// The run is over: remove the file so a finished run never looks live.
    pub fn close(mut self) {
        self.out = None;
        let _ = fs::remove_file(&self.file);
    }

    /// The run is over but someone follows the file (`fm run`): close it and leave it for them to
    /// read to the end and remove. A view only counts as live while the run lock is held.
    pub fn keep(mut self) {
        self.out = None;
    }
}

/// The events in a live file, in order. Lines that do not read are skipped; no file is none.
pub fn read_live(file: &Path) -> Vec<LiveEvent> {
    fs::read(file)
        .map(|b| {
            String::from_utf8_lossy(&b)
                .lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn partial(text: &str, ts: u64) -> LiveEvent {
        LiveEvent::Partial {
            text: text.into(),
            ts,
        }
    }

    fn texts(file: &Path) -> Vec<String> {
        read_live(file)
            .into_iter()
            .map(|e| match e {
                LiveEvent::Partial { text, .. } => format!("partial:{text}"),
                LiveEvent::Thinking { text, .. } => format!("thinking:{text}"),
                LiveEvent::Message { text, .. } => format!("message:{text}"),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn writes_the_first_partial_at_once_then_throttles() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("locks").join("x.live");
        let mut w = LiveWriter::create(&file);
        w.write(partial("a", 1000));
        w.write(partial("ab", 1500));
        w.write(partial("abc", 2000));
        w.write(partial("abc", 3500));
        w.write(partial("   ", 5000));
        w.write(partial("abcd", 5000));
        assert_eq!(texts(&file), ["partial:a", "partial:abc", "partial:abcd"]);
    }

    #[test]
    fn a_finished_message_lets_the_next_partial_through() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("x.live");
        let mut w = LiveWriter::create(&file);
        w.write(partial("draft", 1000));
        w.write(LiveEvent::Message {
            text: "final".into(),
            ts: 1100,
        });
        w.write(LiveEvent::Thinking {
            text: "next".into(),
            ts: 1200,
        });
        assert_eq!(
            texts(&file),
            ["partial:draft", "message:final", "thinking:next"]
        );
    }

    #[test]
    fn bounds_what_it_keeps() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("x.live");
        let mut w = LiveWriter::create(&file);
        let long = format!("{}END", "x".repeat(PARTIAL_MAX * 2));
        w.write(partial(&long, 1));
        w.write(LiveEvent::Message {
            text: format!("START{}", "y".repeat(MESSAGE_MAX * 2)),
            ts: 2,
        });
        w.write(LiveEvent::Tool {
            name: "bash".into(),
            summary: format!("line one\nline two {}", "z".repeat(500)),
            ts: 3,
        });
        let events = read_live(&file);
        match &events[0] {
            LiveEvent::Partial { text, .. } => {
                assert_eq!(text.chars().count(), PARTIAL_MAX);
                assert!(text.ends_with("END"));
            }
            other => panic!("{other:?}"),
        }
        match &events[1] {
            LiveEvent::Message { text, .. } => {
                assert_eq!(text.chars().count(), MESSAGE_MAX);
                assert!(text.starts_with("START"));
            }
            other => panic!("{other:?}"),
        }
        match &events[2] {
            LiveEvent::Tool { summary, .. } => {
                assert!(summary.starts_with("line one line two"));
                assert_eq!(summary.chars().count(), SUMMARY_MAX);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn starts_afresh_and_close_removes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("x.live");
        fs::write(&file, "left over\n").unwrap();
        let mut w = LiveWriter::create(&file);
        w.write(LiveEvent::Session {
            id: "s".into(),
            ts: 1,
        });
        assert_eq!(
            read_live(&file),
            [LiveEvent::Session {
                id: "s".into(),
                ts: 1
            }]
        );
        w.close();
        assert!(!file.exists());
        assert!(read_live(&file).is_empty());
    }

    #[test]
    fn keep_leaves_every_event_for_the_follower() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("x.live");
        let mut w = LiveWriter::create(&file);
        w.write(LiveEvent::Session {
            id: "s".into(),
            ts: 1,
        });
        w.keep();
        assert_eq!(read_live(&file).len(), 1);
        fs::remove_file(&file).unwrap();
    }

    #[test]
    fn never_fails_when_the_file_cannot_be_written() {
        let dir = tempfile::tempdir().unwrap();
        // A directory where the file should be: opening it fails.
        let file = dir.path().join("x.live");
        fs::create_dir_all(&file).unwrap();
        let mut w = LiveWriter::create(&file);
        w.write(partial("a", 1));
        w.close();
        assert!(file.is_dir());
    }
}
