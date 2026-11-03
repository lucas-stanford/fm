//! A fake agent CLI for tests: replays a fixture to stdout and exits with a chosen code.
//!
//! It accepts any arguments, so every adapter can be pointed at it: set the CLI's
//! `[agents.<cli>] path` to the `fm-fake-agent` binary and pass its own options through
//! `extra-args` (which every adapter appends):
//!
//! ```toml
//! [agents.claude]
//! path = "target/debug/fm-fake-agent"
//! extra-args = ["--fm-fake-fixture=crates/fm-agents/tests/fixtures/claude/pong.jsonl"]
//! ```
//!
//! Options (anywhere in the arguments; everything else is ignored). Each also reads an
//! environment variable, used when the option is absent:
//!
//! | option                          | env                 | effect                                          |
//! |---------------------------------|---------------------|-------------------------------------------------|
//! | `--fm-fake-fixture=<path>`      | `FM_FAKE_FIXTURE`   | lines written to stdout, in order               |
//! | `--fm-fake-exit=<code>`         | `FM_FAKE_EXIT`      | exit code (default 0)                           |
//! | `--fm-fake-stderr=<text>`       | `FM_FAKE_STDERR`    | written to stderr before exiting                |
//! | `--fm-fake-delay-ms=<n>`        | `FM_FAKE_DELAY_MS`  | sleep before each stdout line                   |
//! | `--fm-fake-hang`                | `FM_FAKE_HANG=1`    | after replaying, sleep for an hour (timeouts)   |
//! | `--fm-fake-record=<path>`       | `FM_FAKE_RECORD`    | write the arguments and stdin to this file      |
//!
//! `--version` prints `fm-fake-agent <version>` and exits 0, so detection works.
//!
//! The record file holds one argument per line, then a line `--- stdin ---`, then stdin as read.
//! stdin is always read to the end first (the adapters write the prompt there and close it).

use std::io::{BufRead, IsTerminal, Read, Write};
use std::time::Duration;

/// The marker between arguments and stdin in a record file.
pub const RECORD_STDIN_MARKER: &str = "--- stdin ---";

#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    fixture: Option<String>,
    exit: i32,
    stderr: Option<String>,
    delay_ms: u64,
    hang: bool,
    record: Option<String>,
    version: bool,
}

fn options(args: &[String], env: &dyn Fn(&str) -> Option<String>) -> Options {
    let flag = |name: &str| -> Option<String> {
        let prefix = format!("--fm-fake-{name}=");
        args.iter()
            .rev()
            .find_map(|a| a.strip_prefix(&prefix).map(str::to_string))
    };
    let pick = |name: &str, var: &str| flag(name).or_else(|| env(var));
    Options {
        fixture: pick("fixture", "FM_FAKE_FIXTURE"),
        exit: pick("exit", "FM_FAKE_EXIT")
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0),
        stderr: pick("stderr", "FM_FAKE_STDERR"),
        delay_ms: pick("delay-ms", "FM_FAKE_DELAY_MS")
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0),
        hang: args.iter().any(|a| a == "--fm-fake-hang")
            || env("FM_FAKE_HANG").is_some_and(|v| v == "1" || v == "true"),
        record: pick("record", "FM_FAKE_RECORD"),
        version: args.iter().any(|a| a == "--version" || a == "-v"),
    }
}

/// Run the fake agent. Returns the exit code.
pub fn run(
    args: &[String],
    env: &dyn Fn(&str) -> Option<String>,
    stdin: &mut dyn Read,
    stdin_is_terminal: bool,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let opts = options(args, env);
    if opts.version {
        let _ = writeln!(stdout, "fm-fake-agent {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }

    let mut input = Vec::new();
    if !stdin_is_terminal {
        let _ = stdin.read_to_end(&mut input);
    }
    if let Some(path) = &opts.record {
        let mut text = String::new();
        for a in args {
            text.push_str(a);
            text.push('\n');
        }
        text.push_str(RECORD_STDIN_MARKER);
        text.push('\n');
        text.push_str(&String::from_utf8_lossy(&input));
        if let Err(e) = std::fs::write(path, text) {
            let _ = writeln!(stderr, "fm-fake-agent: cannot write {path}: {e}");
            return 97;
        }
    }

    if let Some(path) = &opts.fixture {
        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) => {
                let _ = writeln!(stderr, "fm-fake-agent: cannot read {path}: {e}");
                return 98;
            }
        };
        for line in std::io::BufReader::new(file).lines() {
            let Ok(line) = line else { break };
            if opts.delay_ms > 0 {
                std::thread::sleep(Duration::from_millis(opts.delay_ms));
            }
            if writeln!(stdout, "{}", line.trim_end_matches('\r')).is_err() {
                break;
            }
            let _ = stdout.flush();
        }
    }
    if let Some(text) = &opts.stderr {
        let _ = writeln!(stderr, "{text}");
    }
    let _ = stdout.flush();
    let _ = stderr.flush();
    if opts.hang {
        std::thread::sleep(Duration::from_secs(3600));
    }
    opts.exit
}

/// The whole program: process arguments, environment and standard streams.
pub fn main() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let stdin = std::io::stdin();
    let is_terminal = stdin.is_terminal();
    run(
        &args,
        &|k| std::env::var(k).ok(),
        &mut stdin.lock(),
        is_terminal,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn options_from_args_then_env() {
        let args: Vec<String> = [
            "-p",
            "--fm-fake-exit=3",
            "--fm-fake-fixture=a.jsonl",
            "--fm-fake-hang",
        ]
        .map(String::from)
        .to_vec();
        let o = options(&args, &no_env);
        assert_eq!(o.exit, 3);
        assert_eq!(o.fixture.as_deref(), Some("a.jsonl"));
        assert!(o.hang);
        let env = |k: &str| (k == "FM_FAKE_EXIT").then(|| "7".to_string());
        assert_eq!(options(&[], &env).exit, 7);
        assert_eq!(options(&args, &env).exit, 3);
    }

    #[test]
    fn replays_fixture_records_and_exits() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = dir.path().join("f.jsonl");
        std::fs::write(&fixture, "{\"a\":1}\r\nsecond\n").unwrap();
        let record = dir.path().join("rec.txt");
        let args = vec![
            "--model".to_string(),
            format!("--fm-fake-fixture={}", fixture.display()),
            format!("--fm-fake-record={}", record.display()),
            "--fm-fake-exit=4".to_string(),
            "--fm-fake-stderr=oops".to_string(),
        ];
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(
            &args,
            &no_env,
            &mut "the prompt".as_bytes(),
            false,
            &mut out,
            &mut err,
        );
        assert_eq!(code, 4);
        assert_eq!(String::from_utf8(out).unwrap(), "{\"a\":1}\nsecond\n");
        assert_eq!(String::from_utf8(err).unwrap(), "oops\n");
        let rec = std::fs::read_to_string(record).unwrap();
        assert!(rec.starts_with("--model\n"));
        assert!(rec.ends_with(&format!("{RECORD_STDIN_MARKER}\nthe prompt")));
    }

    #[test]
    fn version_and_missing_fixture() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(
            &["--version".into()],
            &no_env,
            &mut "".as_bytes(),
            false,
            &mut out,
            &mut err,
        );
        assert_eq!(code, 0);
        assert!(
            String::from_utf8(out)
                .unwrap()
                .starts_with("fm-fake-agent ")
        );
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(
            &["--fm-fake-fixture=does-not-exist.jsonl".into()],
            &no_env,
            &mut "".as_bytes(),
            true,
            &mut out,
            &mut err,
        );
        assert_eq!(code, 98);
    }
}
