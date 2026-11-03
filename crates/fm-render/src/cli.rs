//! Argument parsing and I/O for `fm render`.

use std::io::{Read, Write};
use std::process::ExitCode;

use crate::error::{Code, Failure, Result, fail};
use crate::flow::parse;
use crate::render::{svg, text, theme::Theme};
use crate::state::State;

const SHORT_USAGE: &str = "Usage: fm render --plan <file|-> [--state <file|->] [--format text|svg] [--theme dark|light] [--out <file>]";

const USAGE: &str = "\
fm draws a Mermaid flowchart with node statuses, as SVG or text.

Usage: fm render --plan <file|-> [--state <file|->] [--format text|svg] [--theme dark|light] [--out <file>]

Commands:
  render  Draw a flowchart with node statuses

Options:
  -h, --help     Print help
  -V, --version  Print version

Run 'fm render --help' for details.
";

const RENDER_HELP: &str = "\
Draw a Mermaid flowchart with node statuses, as text or SVG.

Usage: fm render --plan <file|-> [--state <file|->] [--format text|svg] [--theme dark|light] [--out <file>]

Options:
  --plan <file|->    Mermaid flowchart to draw; '-' reads stdin (required)
  --state <file|->   JSON with node statuses and arrows taken; '-' reads stdin
  --format <format>  text (default) or svg
  --theme <theme>    dark (default) or light; applies to svg
  --out <file>       Write the rendering to a file instead of stdout
  -h, --help         Print this help

State JSON:
  {\"nodes\": {\"<nodeId>\": \"<status>\", ...}, \"taken\": [[\"<from>\", \"<to>\"], ...]}
  status is one of: pending, active, waiting, done, failed, skipped, interrupted
  A node that is not listed is pending. Unknown fields are ignored; an unknown
  node id, status or arrow is an error.

Exit status: 0 success, 1 I/O or render failure, 2 bad arguments, flowchart or state.
Supported Mermaid: docs/SUPPORTED_MERMAID.md
";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Format {
    #[default]
    Text,
    Svg,
}

#[derive(Debug, Default)]
struct Options {
    plan: String,
    state: Option<String>,
    format: Format,
    theme: Theme,
    out: Option<String>,
}

enum Invocation {
    Print(&'static str),
    Render(Options),
}

/// `fm <argv…>` where `argv[0]` is `render`, `--help` or `--version`: parse, draw and print.
pub fn render_command(argv: &[String]) -> ExitCode {
    ExitCode::from(render_status(argv))
}

/// [`render_command`], as the exit status: 0 success, 1 I/O or render failure, 2 bad input.
pub fn render_status(argv: &[String]) -> u8 {
    match run(argv) {
        Ok(()) => 0,
        Err(failure) => {
            eprintln!("fm: {failure}");
            if failure.code == Code::Usage {
                eprintln!("\n{SHORT_USAGE}\nRun 'fm render --help' for details.");
            }
            failure.code.exit()
        }
    }
}

fn run(argv: &[String]) -> Result<()> {
    match parse_args(argv)? {
        Invocation::Print(text) => write_stdout(text),
        Invocation::Render(options) => {
            let rendered = render(&options)?;
            match &options.out {
                Some(path) => std::fs::write(path, rendered)
                    .map_err(|error| fail!(Code::Io, "cannot write '{path}': {error}")),
                None => write_stdout(&rendered),
            }
        }
    }
}

fn parse_args(argv: &[String]) -> Result<Invocation> {
    let Some(command) = argv.first() else {
        return Err(fail!(Code::Usage, "no command given"));
    };
    match command.as_str() {
        "-h" | "--help" | "help" => return Ok(Invocation::Print(USAGE)),
        "-V" | "--version" => {
            let version = concat!("fm ", env!("CARGO_PKG_VERSION"), "\n");
            return Ok(Invocation::Print(version));
        }
        "render" => {}
        other => return Err(fail!(Code::Usage, "unknown command '{other}'")),
    }

    let mut plan = None;
    let mut options = Options::default();
    let mut format = None;
    let mut theme = None;
    let mut rest = argv[1..].iter();
    while let Some(arg) = rest.next() {
        if arg == "-h" || arg == "--help" {
            return Ok(Invocation::Print(RENDER_HELP));
        }
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name, Some(value.to_string())),
            _ => (arg.as_str(), None),
        };
        let slot = match name {
            "--plan" => &mut plan,
            "--state" => &mut options.state,
            "--format" => &mut format,
            "--theme" => &mut theme,
            "--out" => &mut options.out,
            _ if name.starts_with('-') && name != "-" => {
                return Err(fail!(Code::Usage, "unknown option '{name}'"));
            }
            _ => return Err(fail!(Code::Usage, "unexpected argument '{arg}'")),
        };
        if slot.is_some() {
            return Err(fail!(Code::Usage, "{name} given more than once"));
        }
        let value = match inline {
            Some(value) => value,
            None => rest
                .next()
                .cloned()
                .ok_or_else(|| fail!(Code::Usage, "{name} needs a value"))?,
        };
        if value.is_empty() {
            return Err(fail!(Code::Usage, "{name} needs a value"));
        }
        *slot = Some(value);
    }

    options.plan = plan.ok_or_else(|| fail!(Code::Usage, "--plan is required"))?;
    if options.plan == "-" && options.state.as_deref() == Some("-") {
        return Err(fail!(
            Code::Usage,
            "--plan and --state cannot both read stdin"
        ));
    }
    options.format = match format.as_deref() {
        None | Some("text") => Format::Text,
        Some("svg") => Format::Svg,
        Some(other) => {
            return Err(fail!(
                Code::Usage,
                "--format must be text or svg, not '{other}'"
            ));
        }
    };
    if let Some(value) = theme {
        options.theme = Theme::parse(&value)
            .ok_or_else(|| fail!(Code::Usage, "--theme must be dark or light, not '{value}'"))?;
    }
    Ok(Invocation::Render(options))
}

fn render(options: &Options) -> Result<String> {
    let plan = read_source(&options.plan, "plan")?;
    let flow = parse::parse(&plan).map_err(|failure| prefixed("plan", failure))?;
    parse::validate_renderable(&plan).map_err(|failure| prefixed("plan", failure))?;
    let state = match &options.state {
        Some(source) => State::parse(&read_source(source, "state")?, &flow)?,
        None => State::default(),
    };

    let mut rendered = match options.format {
        Format::Text => text::render(&plan, &flow, &state)?,
        Format::Svg => svg::render(&plan, &flow, &state, options.theme)?,
    };
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    Ok(rendered)
}

fn prefixed(what: &str, failure: Failure) -> Failure {
    Failure::new(failure.code, format!("{what}: {}", failure.message))
}

/// Reads a file, or stdin for `-`. A UTF-8 byte order mark is dropped, because
/// Windows tools add one and neither Mermaid nor JSON allows it.
fn read_source(source: &str, what: &str) -> Result<String> {
    let mut text = String::new();
    if source == "-" {
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|error| fail!(Code::Io, "cannot read {what} from stdin: {error}"))?;
    } else {
        text = std::fs::read_to_string(source)
            .map_err(|error| fail!(Code::Io, "cannot read {what} '{source}': {error}"))?;
    }
    Ok(match text.strip_prefix('\u{feff}') {
        Some(stripped) => stripped.to_string(),
        None => text,
    })
}

fn write_stdout(text: &str) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
        .map_err(|error| fail!(Code::Io, "cannot write to stdout: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    fn options(list: &[&str]) -> Result<Options> {
        match parse_args(&args(list))? {
            Invocation::Render(options) => Ok(options),
            Invocation::Print(_) => panic!("expected a render invocation"),
        }
    }

    #[test]
    fn defaults_are_text_and_dark() {
        let parsed = options(&["render", "--plan", "p.mmd"]).expect("parse");
        assert_eq!(parsed.plan, "p.mmd");
        assert_eq!(parsed.format, Format::Text);
        assert_eq!(parsed.theme, Theme::Dark);
        assert!(parsed.state.is_none() && parsed.out.is_none());
    }

    #[test]
    fn every_option_is_read_in_both_spellings() {
        let parsed = options(&[
            "render",
            "--plan=-",
            "--state",
            "s.json",
            "--format=svg",
            "--theme",
            "light",
            "--out",
            "o.svg",
        ])
        .expect("parse");
        assert_eq!(parsed.plan, "-");
        assert_eq!(parsed.state.as_deref(), Some("s.json"));
        assert_eq!(parsed.format, Format::Svg);
        assert_eq!(parsed.theme, Theme::Light);
        assert_eq!(parsed.out.as_deref(), Some("o.svg"));
    }

    #[test]
    fn bad_invocations_are_usage_errors() {
        for list in [
            &["render"][..],
            &["render", "--plan"],
            &["render", "--plan", "a", "--plan", "b"],
            &["render", "--plan", "a", "--format", "png"],
            &["render", "--plan", "a", "--theme", "sepia"],
            &["render", "--plan", "a", "--bogus", "x"],
            &["render", "--plan", "a", "extra"],
            &["render", "--plan", "-", "--state", "-"],
            &["status"],
            &[],
        ] {
            let error = parse_args(&args(list))
                .err()
                .unwrap_or_else(|| panic!("{list:?} was accepted"));
            assert_eq!(error.code, Code::Usage, "{list:?}");
        }
    }

    #[test]
    fn help_and_version_print() {
        for (list, expected) in [
            (&["--help"][..], "Usage: fm render"),
            (&["render", "--help"], "State JSON:"),
            (&["render", "--plan", "x", "-h"], "State JSON:"),
            (&["--version"], "fm "),
        ] {
            match parse_args(&args(list)).expect("parse") {
                Invocation::Print(text) => assert!(text.contains(expected), "{list:?}"),
                Invocation::Render(_) => panic!("{list:?} did not print"),
            }
        }
    }
}
