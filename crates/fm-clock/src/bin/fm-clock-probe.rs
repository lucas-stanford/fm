//! A stand-in for `fm` in fm-clock's own tests, implementing just the two commands the clock
//! relies on:
//!
//! - `tick --by job|serve|cli`: one real tick against `FM_HOME`, discovering under
//!   `<FM_HOME>/home` (never the real home directory), starting runs with [`DetachedSpawn`]
//!   pointed back at this executable.
//! - `fire --key <key> --trigger <t> [--scheduled-for <iso>] [--late]`: waits
//!   `FM_PROBE_FIRE_DELAY_MS` (default 1500, so it outlives the tick that started it), then appends
//!   its arguments as one line to `<FM_HOME>/fired.txt`.

use std::io::Write;

use fm_clock::{DetachedSpawn, TickContext, tick};
use fm_model::TickSource;

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let paths = match fm_state::state_paths() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("fm-clock-probe: {e}");
            return 2;
        }
    };
    match args.first().map(String::as_str) {
        Some("tick") => {
            let by = match args
                .iter()
                .skip_while(|a| *a != "--by")
                .nth(1)
                .map(String::as_str)
            {
                Some("job") => TickSource::Job,
                Some("serve") => TickSource::Serve,
                _ => TickSource::Cli,
            };
            let exe = match std::env::current_exe() {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("fm-clock-probe: {e}");
                    return 2;
                }
            };
            let starter = DetachedSpawn::new(exe).with_log_dir(paths.logs_dir());
            let home = paths.root.join("home");
            let ctx = TickContext::new(paths, home, by, &starter);
            match tick(&ctx, chrono::Utc::now()) {
                Ok(r) => {
                    if r.eventful() {
                        println!(
                            "{} {}",
                            fm_state::time::iso(chrono::Utc::now()),
                            r.summary()
                        );
                    }
                    0
                }
                Err(e) => {
                    eprintln!("fm-clock-probe: tick failed: {e}");
                    1
                }
            }
        }
        Some("fire") => {
            let delay = std::env::var("FM_PROBE_FIRE_DELAY_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1500);
            std::thread::sleep(std::time::Duration::from_millis(delay));
            let file = paths.root.join("fired.txt");
            let line = format!("{}\n", args.join(" "));
            match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&file)
                .and_then(|mut f| f.write_all(line.as_bytes()))
            {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("fm-clock-probe: {}: {e}", file.display());
                    1
                }
            }
        }
        _ => {
            eprintln!("fm-clock-probe: use tick --by <job|serve|cli>, or fire --key <key> …");
            2
        }
    }
}
