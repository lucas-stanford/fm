//! Opening `AUTOMATION.md` in the person's editor: `$VISUAL`, then `$EDITOR`, then `notepad` on
//! Windows, `nano` then `vi` elsewhere. The terminal is handed over while the editor runs.

use std::io;
use std::path::Path;
use std::process::{Command, ExitStatus};

/// Leaves and re-enters the full-screen UI around a program that needs the terminal.
pub trait Suspend {
    fn suspend(&mut self) -> io::Result<()>;
    fn resume(&mut self) -> io::Result<()>;
}

/// `code --wait` → `["code", "--wait"]`. Single and double quotes group words; no expansion.
pub fn split_command(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut any = false;
    for c in s.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                any = true;
            }
            None if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            None => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The editors to try, in order. `env` reads the environment; `windows` picks the fallback.
pub fn editor_candidates(env: &dyn Fn(&str) -> Option<String>, windows: bool) -> Vec<Vec<String>> {
    for var in ["VISUAL", "EDITOR"] {
        if let Some(v) = env(var) {
            let words = split_command(&v);
            if !words.is_empty() {
                return vec![words];
            }
        }
    }
    if windows {
        vec![vec!["notepad".into()]]
    } else {
        vec![vec!["nano".into()], vec!["vi".into()]]
    }
}

/// Run the first editor that exists on `path` and wait for it. `Err` is one line for the status bar.
pub fn launch(candidates: &[Vec<String>], path: &Path) -> Result<(), String> {
    let mut tried = Vec::new();
    for words in candidates {
        let Some((program, args)) = words.split_first() else {
            continue;
        };
        match Command::new(program).args(args).arg(path).status() {
            Ok(status) => return exit_words(program, status),
            Err(e) if e.kind() == io::ErrorKind::NotFound => tried.push(program.clone()),
            Err(e) => return Err(format!("{program} could not start: {e}")),
        }
    }
    Err(format!(
        "no editor found (tried {}) — set $EDITOR",
        tried.join(", ")
    ))
}

fn exit_words(program: &str, status: ExitStatus) -> Result<(), String> {
    if status.success() {
        Ok(())
    } else {
        Err(match status.code() {
            Some(code) => format!("{program} exited {code}"),
            None => format!("{program} was stopped"),
        })
    }
}

/// Leave the UI, run `edit`, and come back whatever happened. The editor's own answer is returned;
/// a terminal that cannot be restored is the only `io::Error`.
pub fn with_terminal_released(
    screen: &mut dyn Suspend,
    edit: &mut dyn FnMut() -> Result<(), String>,
) -> io::Result<Result<(), String>> {
    if let Err(e) = screen.suspend() {
        // Half-left is worse than not leaving: get back to the UI and say why.
        screen.resume()?;
        return Ok(Err(format!("could not hand the terminal over: {e}")));
    }
    let answer = edit();
    screen.resume()?;
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn visual_before_editor() {
        let c = editor_candidates(&env(&[("VISUAL", "code --wait"), ("EDITOR", "vim")]), false);
        assert_eq!(c, vec![vec!["code".to_string(), "--wait".into()]]);
        let c = editor_candidates(&env(&[("EDITOR", "vim")]), true);
        assert_eq!(c, vec![vec!["vim".to_string()]]);
    }

    #[test]
    fn fallbacks_per_os() {
        assert_eq!(
            editor_candidates(&env(&[]), true),
            vec![vec!["notepad".to_string()]]
        );
        assert_eq!(
            editor_candidates(&env(&[("EDITOR", "  ")]), false),
            vec![vec!["nano".to_string()], vec!["vi".to_string()]]
        );
    }

    #[test]
    fn split_keeps_quoted_paths() {
        assert_eq!(
            split_command(r#""C:\Program Files\Editor\ed.exe" -w"#),
            vec![r"C:\Program Files\Editor\ed.exe".to_string(), "-w".into()]
        );
        assert_eq!(split_command("subl -n -w"), vec!["subl", "-n", "-w"]);
        assert_eq!(split_command("a ''"), vec!["a", ""]);
    }

    #[test]
    fn missing_editor_says_so() {
        let err = launch(
            &[
                vec!["fm-tui-no-such-editor-1".into()],
                vec!["fm-tui-no-such-editor-2".into()],
            ],
            Path::new("AUTOMATION.md"),
        )
        .unwrap_err();
        assert!(
            err.contains("fm-tui-no-such-editor-1, fm-tui-no-such-editor-2"),
            "{err}"
        );
    }

    #[derive(Default)]
    struct Recorder(Vec<&'static str>);
    impl Suspend for Recorder {
        fn suspend(&mut self) -> io::Result<()> {
            self.0.push("suspend");
            Ok(())
        }
        fn resume(&mut self) -> io::Result<()> {
            self.0.push("resume");
            Ok(())
        }
    }

    #[test]
    fn terminal_comes_back_even_when_the_editor_fails() {
        let mut screen = Recorder::default();
        let answer =
            with_terminal_released(&mut screen, &mut || Err("vim exited 1".into())).unwrap();
        assert_eq!(answer, Err("vim exited 1".into()));
        assert_eq!(screen.0, vec!["suspend", "resume"]);
    }

    #[test]
    fn a_real_editor_process_runs_and_is_waited_for() {
        let dir = std::env::temp_dir().join(format!("fm-tui-editor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("AUTOMATION.md");
        std::fs::write(&file, "before\n").unwrap();
        // A stand-in editor that appends a line to the file it is given.
        let editor: Vec<String> = if cfg!(windows) {
            vec!["cmd".into(), "/C".into(), "echo edited>>".into()]
        } else {
            vec!["sh".into(), "-c".into(), "echo edited >> \"$0\"".into()]
        };
        launch(&[editor], &file).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("edited"), "{text:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
