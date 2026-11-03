//! Opening a run's link in the browser, without a shell in between: a link comes from an agent, so
//! it is never parsed by `cmd` or `sh`.

use std::process::{Command, Stdio};

pub trait Opener {
    /// Open `url`; `Err` is one line for the status bar.
    fn open(&mut self, url: &str) -> Result<(), String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    MacOs,
    Other,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Other
        }
    }
}

/// Only http and https, with no control characters or spaces (run links are always http(s)).
pub fn safe_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    (lower.starts_with("https://") || lower.starts_with("http://"))
        && url.len() > 8
        && !url
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '"')
}

/// The program and arguments that open `url`, or `None` for a link that is not safe to hand over.
/// Windows uses `url.dll` directly: `start` would need `cmd`, which treats `&` in a query string as
/// a second command.
pub fn open_command(url: &str, os: Os) -> Option<(String, Vec<String>)> {
    if !safe_url(url) {
        return None;
    }
    Some(match os {
        Os::Windows => (
            "rundll32".into(),
            vec!["url.dll,FileProtocolHandler".into(), url.into()],
        ),
        Os::MacOs => ("open".into(), vec![url.into()]),
        Os::Other => ("xdg-open".into(), vec![url.into()]),
    })
}

/// Opens links with the system's own handler.
#[derive(Debug, Default)]
pub struct SystemOpener;

impl Opener for SystemOpener {
    fn open(&mut self, url: &str) -> Result<(), String> {
        let (program, args) = open_command(url, Os::current())
            .ok_or_else(|| format!("not opening {url} — only http and https links open"))?;
        let mut child = Command::new(&program)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("{program} could not start: {e}"))?;
        // Reap it without making the UI wait for the browser.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

/// Remembers what it was asked to open, for tests.
#[derive(Debug, Default)]
pub struct RecordingOpener {
    pub opened: Vec<String>,
}

impl Opener for RecordingOpener {
    fn open(&mut self, url: &str) -> Result<(), String> {
        self.opened.push(url.to_string());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_per_os() {
        let url = "https://example.com/a?b=1&c=2";
        assert_eq!(
            open_command(url, Os::Windows),
            Some((
                "rundll32".into(),
                vec!["url.dll,FileProtocolHandler".into(), url.into()]
            ))
        );
        assert_eq!(open_command(url, Os::MacOs).unwrap().0, "open");
        assert_eq!(open_command(url, Os::Other).unwrap().0, "xdg-open");
    }

    #[test]
    fn refuses_what_is_not_a_web_link() {
        for bad in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://x.com/\nrm",
            "https://a b",
            "http://",
            "calc.exe",
        ] {
            assert_eq!(open_command(bad, Os::Other), None, "{bad}");
        }
        assert!(SystemOpener.open("file:///etc/passwd").is_err());
    }
}
