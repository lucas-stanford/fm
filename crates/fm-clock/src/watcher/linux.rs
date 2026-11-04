//! Linux: a systemd user timer that runs one tick every minute, or a crontab line where there is
//! no systemd user session (containers, WSL without systemd, minimal servers).
//!
//! The service is `Type=oneshot` running `/bin/sh <state>/watcher/fm-tick.sh` (see
//! [`super::unix`]), with `KillMode=process` so the runs the tick started outlive it (systemd
//! otherwise kills everything left in the unit's cgroup when the tick exits). The timer fires on
//! every minute, on the minute, and `Persistent=true` fires once after downtime rather than once
//! per missed minute. A user timer stops at logout unless lingering is on
//! (`loginctl enable-linger`); `fm watcher status` says so.

use std::path::Path;

use super::unix::sh_quote;
use super::{Cmd, JobSpec};

/// A value in a unit file's quoted string: `\`, `"` escaped, `%` (a specifier) doubled.
fn unit_quote(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', r"\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
    )
}

/// `<unit>.service`.
pub fn service_unit(spec: &JobSpec) -> String {
    let env: String = spec
        .env
        .iter()
        .map(|(k, v)| format!("Environment={}\n", unit_quote(&format!("{k}={v}"))))
        .collect();
    format!(
        "[Unit]\n\
         Description=Agent Automations (fm): one tick of the watcher, which fires schedules and runs watch checks\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         ExecStart=/bin/sh {script}\n\
         KillMode=process\n\
         {env}",
        script = unit_quote(&spec.script.to_string_lossy()).replace('$', "$$"),
    )
}

/// `<unit>.timer`.
pub fn timer_unit(unit: &str) -> String {
    format!(
        "[Unit]\n\
         Description=Agent Automations (fm): the watcher ticks once a minute\n\
         \n\
         [Timer]\n\
         OnCalendar=*-*-* *:*:00\n\
         AccuracySec=1s\n\
         Persistent=true\n\
         Unit={unit}.service\n\
         \n\
         [Install]\n\
         WantedBy=timers.target\n"
    )
}

fn systemctl(args: &[&str]) -> Cmd {
    let mut all = vec!["--user".to_string()];
    all.extend(args.iter().map(|s| s.to_string()));
    Cmd::new("systemctl", all)
}

/// Is there a systemd user session to register with?
pub fn probe_systemd() -> Cmd {
    systemctl(&["show-environment"])
}

pub fn register(unit: &str) -> Vec<Cmd> {
    vec![
        systemctl(&["daemon-reload"]),
        systemctl(&["enable", "--now", &format!("{unit}.timer")]),
    ]
}

pub fn run_now(unit: &str) -> Cmd {
    systemctl(&["start", "--no-block", &format!("{unit}.service")])
}

/// Disable the timer. The unit files are removed afterwards, then [`reload`].
pub fn remove(unit: &str) -> Vec<Cmd> {
    vec![systemctl(&["disable", "--now", &format!("{unit}.timer")])]
}

pub fn reload() -> Cmd {
    systemctl(&["daemon-reload"])
}

pub fn query(unit: &str) -> Cmd {
    systemctl(&["is-active", &format!("{unit}.timer")])
}

/// Shell lines the wrapper runs to remove the timer once the executable is gone.
pub fn unregister_lines(unit: &str, files: &[&Path]) -> Vec<String> {
    let mut out = vec![format!(
        "systemctl --user disable --now {} >/dev/null 2>&1",
        sh_quote(&format!("{unit}.timer"))
    )];
    let quoted: Vec<String> = files
        .iter()
        .map(|f| sh_quote(&f.to_string_lossy()))
        .collect();
    out.push(format!("rm -f {}", quoted.join(" ")));
    out.push("systemctl --user daemon-reload >/dev/null 2>&1".to_string());
    out
}

// ------------------------------------------------------------------------------------- cron

/// The stable comment that marks the watcher's line in a crontab.
pub fn cron_marker(unit: &str) -> String {
    format!("# {unit} (fm watcher)")
}

/// `%` is a newline in a crontab command.
fn cron_escape(s: &str) -> String {
    s.replace('%', r"\%")
}

/// The crontab line used when there is no systemd user session.
pub fn crontab_line(spec: &JobSpec) -> String {
    format!(
        "* * * * * /bin/sh {} >> {} 2>&1 {}",
        cron_escape(&sh_quote(&spec.script.to_string_lossy())),
        cron_escape(&sh_quote(&spec.log.to_string_lossy())),
        cron_marker(&spec.names.unit)
    )
}

/// `existing` without the watcher's line.
pub fn crontab_without(existing: &str, unit: &str) -> String {
    let marker = cron_marker(unit);
    let mut out: String = existing
        .lines()
        .filter(|l| !l.ends_with(&marker))
        .map(|l| format!("{l}\n"))
        .collect();
    if out.trim().is_empty() {
        out.clear();
    }
    out
}

/// `existing` with the watcher's line, replacing an older one.
pub fn crontab_with(existing: &str, line: &str, unit: &str) -> String {
    let mut out = crontab_without(existing, unit);
    out.push_str(line);
    out.push('\n');
    out
}

pub fn crontab_list() -> Cmd {
    Cmd::new("crontab", vec!["-l".into()])
}

pub fn crontab_install(text: &str) -> Cmd {
    let mut c = Cmd::new("crontab", vec!["-".into()]);
    c.stdin = Some(text.to_string());
    c
}

/// Shell lines the wrapper runs to remove the crontab line once the executable is gone.
pub fn cron_unregister_lines(unit: &str) -> Vec<String> {
    vec![format!(
        "crontab -l 2>/dev/null | grep -v -F {} | crontab -",
        sh_quote(&cron_marker(unit))
    )]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watcher::{JobSpec, Names};
    use std::path::PathBuf;

    fn spec() -> JobSpec {
        JobSpec {
            exe: PathBuf::from("/home/me/.cargo/bin/fm"),
            state_dir: PathBuf::from("/home/me/.local/state/agent-automations"),
            env: vec![
                (
                    "FM_HOME".into(),
                    "/home/me/.local/state/agent-automations".into(),
                ),
                ("PATH".into(), "/home/me/.cargo/bin:/usr/bin".into()),
                ("LANG".into(), "C.UTF-8".into()),
            ],
            names: Names::default(),
            script: PathBuf::from("/home/me/.local/state/agent-automations/watcher/fm-tick.sh"),
            log: PathBuf::from("/home/me/.local/state/agent-automations/logs/tick.log"),
            record: PathBuf::from("/home/me/.local/state/agent-automations/watcher.json"),
        }
    }

    #[test]
    fn the_timer_fires_on_every_minute_and_once_after_downtime() {
        let t = timer_unit("fm-tick");
        assert!(t.contains("OnCalendar=*-*-* *:*:00\n"));
        assert!(t.contains("Persistent=true\n"));
        assert!(t.contains("Unit=fm-tick.service\n"));
        assert!(t.contains("[Install]\nWantedBy=timers.target\n"));
    }

    #[test]
    fn the_service_is_a_oneshot_that_leaves_runs_alive() {
        let s = service_unit(&spec());
        assert!(s.contains("Type=oneshot\n"));
        assert!(s.contains(
            "ExecStart=/bin/sh \"/home/me/.local/state/agent-automations/watcher/fm-tick.sh\"\n"
        ));
        assert!(s.contains("KillMode=process\n"));
        assert!(s.contains("Environment=\"PATH=/home/me/.cargo/bin:/usr/bin\"\n"));
        assert!(s.contains("Environment=\"FM_HOME=/home/me/.local/state/agent-automations\"\n"));
    }

    #[test]
    fn the_service_escapes_specifiers_and_quotes() {
        let mut sp = spec();
        sp.script = PathBuf::from("/home/me/100% \"x\"/$y/fm-tick.sh");
        let s = service_unit(&sp);
        assert!(
            s.contains(r#"ExecStart=/bin/sh "/home/me/100%% \"x\"/$$y/fm-tick.sh""#),
            "{s}"
        );
    }

    #[test]
    fn systemctl_commands_are_per_user() {
        assert_eq!(
            register("fm-tick")[1].args,
            ["--user", "enable", "--now", "fm-tick.timer"]
        );
        assert_eq!(
            remove("fm-tick")[0].args,
            ["--user", "disable", "--now", "fm-tick.timer"]
        );
        assert_eq!(
            run_now("fm-tick").args,
            ["--user", "start", "--no-block", "fm-tick.service"]
        );
        assert_eq!(probe_systemd().args, ["--user", "show-environment"]);
    }

    #[test]
    fn falls_back_to_a_crontab_line_that_logs() {
        let line = crontab_line(&spec());
        assert_eq!(
            line,
            "* * * * * /bin/sh '/home/me/.local/state/agent-automations/watcher/fm-tick.sh' >> '/home/me/.local/state/agent-automations/logs/tick.log' 2>&1 # fm-tick (fm watcher)"
        );
        let mut sp = spec();
        sp.log = PathBuf::from("/x/50%/tick.log");
        assert!(crontab_line(&sp).contains(r"'/x/50\%/tick.log'"));
    }

    #[test]
    fn edits_only_its_own_crontab_line() {
        let mine = crontab_line(&spec());
        let existing = "MAILTO=me\n0 * * * * backup\n";
        let with = crontab_with(existing, &mine, "fm-tick");
        assert_eq!(with, format!("MAILTO=me\n0 * * * * backup\n{mine}\n"));
        // Again: replaced, not duplicated.
        assert_eq!(crontab_with(&with, &mine, "fm-tick"), with);
        assert_eq!(crontab_without(&with, "fm-tick"), existing);
        assert_eq!(crontab_without(&format!("{mine}\n"), "fm-tick"), "");
        // Another install's line (pa's, say) is left alone.
        let other = "* * * * * node tick.js # pi-automations\n";
        assert_eq!(crontab_without(other, "fm-tick"), other);
    }
}
