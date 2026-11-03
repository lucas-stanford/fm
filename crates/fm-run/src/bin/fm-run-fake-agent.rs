//! Test-only stand-in for an agent CLI, and for `fm` itself.
//!
//! - `fm-run-fake-agent <verb> …` with a progress verb (`step`, `next`, …) is `fm <verb> …`.
//! - Anything else is `fm_agents::fake` (see its options), after these:
//!   - `--fm-run-progress=<verb>|<arg>|…`: run one progress verb first, as an agent would, with
//!     the run's `FM_RUN`, `FM_NODE` and `FM_STATE_DIR`. Repeatable.
//!   - `--fm-run-log=<path>`: append one JSON line: those variables, the first `PATH` entry, and
//!     each progress verb's exit code.
//!   - `--fm-run-pid=<path>`: write this process's id, so a test can see it die.

use std::io::Write;

use fm_state::Env;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let env = fm_state::ProcessEnv;
    if args.first().is_some_and(|v| fm_run::is_in_run_verb(v)) {
        let out = fm_run::progress_cli(&args, &env, chrono::Utc::now());
        print!("{}", out.stdout);
        eprint!("{}", out.stderr);
        std::process::exit(out.code);
    }

    let flag =
        |name: &str| -> Vec<&str> { args.iter().filter_map(|a| a.strip_prefix(name)).collect() };
    if let Some(path) = flag("--fm-run-pid=").first() {
        let _ = std::fs::write(path, std::process::id().to_string());
    }
    let mut codes = Vec::new();
    for spec in flag("--fm-run-progress=") {
        let argv: Vec<String> = spec.split('|').map(str::to_string).collect();
        let out = fm_run::progress_cli(&argv, &env, chrono::Utc::now());
        eprint!("{}", out.stderr);
        codes.push(out.code);
    }
    if let Some(path) = flag("--fm-run-log=").first() {
        let path_var = std::env::var_os("PATH").unwrap_or_default();
        let first = std::env::split_paths(&path_var)
            .next()
            .map(|p| p.to_string_lossy().into_owned());
        let line = serde_json::json!({
            "FM_RUN": env.var("FM_RUN"),
            "FM_NODE": env.var("FM_NODE"),
            "FM_STATE_DIR": env.var("FM_STATE_DIR"),
            "PATH0": first,
            "codes": codes,
        });
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{line}");
        }
    }
    std::process::exit(fm_agents::fake::main());
}
