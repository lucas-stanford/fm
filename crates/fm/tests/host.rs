//! The real Host on a fixture: state precedence, the pa badge, the preview, live runs, charts.

mod common;

use chrono::{TimeZone, Utc};
use common::{Fx, fm_exe, toml_path};
use fm::host::RealHost;
use fm_model::{AutomationState, CliId, Host, LiveEvent, Trigger};
use fm_state::{Acquire, RunLockFile, SystemProbe, acquire_run_lock, own_lock_info};

fn state(host: &mut RealHost, name: &str) -> (AutomationState, Option<String>) {
    let s = host
        .list()
        .into_iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("{name} not listed"));
    (s.state, s.reason)
}

#[test]
fn state_precedence_invalid_disabled_off_armed() {
    let fx = Fx::new();
    fx.manual("ready");
    fx.write(
        &fx.user_root(),
        "resting",
        "enabled: false\non:\n  manual: true\n",
    );
    fx.write(
        &fx.user_root(),
        "broken",
        "on:\n  schedule: \"61 * * * *\"\n",
    );
    // Disabled wins over off, even with no consent.
    let mut host = fx.host();
    assert_eq!(state(&mut host, "broken").0, AutomationState::Invalid);
    assert!(
        state(&mut host, "broken")
            .1
            .unwrap()
            .starts_with("on.schedule")
    );
    assert_eq!(
        state(&mut host, "resting"),
        (AutomationState::Disabled, None)
    );
    assert_eq!(
        state(&mut host, "ready"),
        (
            AutomationState::Off,
            Some("not armed — fm arm ready".into())
        )
    );
    let key = fx.user_key("ready");
    host.arm(&key).unwrap();
    assert_eq!(state(&mut host, "ready"), (AutomationState::Armed, None));

    // A machine check failing makes an armed automation invalid: claude cannot serve openai.
    std::fs::write(
        fx.user_root().join("ready").join("AUTOMATION.md"),
        "---\nname: ready\ndescription: d\nmodel: openai/gpt-6\non:\n  manual: true\n---\n\nHi.\n",
    )
    .unwrap();
    host.arm(&key).unwrap();
    assert_eq!(state(&mut host, "ready").0, AutomationState::Armed);
    fx.config("default-agent = \"claude\"\n");
    let (st, reason) = state(&mut host, "ready");
    assert_eq!(st, AutomationState::Invalid);
    assert!(reason.unwrap().starts_with("model: "));
    // Off comes before the machine check.
    host.disarm(&key).unwrap();
    assert_eq!(state(&mut host, "ready").0, AutomationState::Off);
}

#[test]
fn armed_schedule_has_a_next_fire_and_watch_a_next_check() {
    let fx = Fx::new();
    let script = if cfg!(windows) { "w.cmd" } else { "w.sh" };
    fx.write(
        &fx.user_root(),
        "both",
        &format!(
            "on:\n  schedule:\n    cron: \"0 9 * * *\"\n    timezone: UTC\n  watch:\n    script: {script}\n    every: 5m\n  manual: true\n"
        ),
    );
    std::fs::write(fx.user_root().join("both").join(script), "echo hi\n").unwrap();
    let now = Utc.with_ymd_and_hms(2026, 10, 29, 8, 0, 0).unwrap();
    let mut env = fx.env();
    env.path_var = std::env::var_os("PATH").unwrap_or_default();
    env.pathext = std::env::var("PATHEXT").ok();
    let mut host = RealHost::new(env).with_now(now);
    let key = fx.user_key("both");
    host.arm(&key).unwrap();
    let s = host.list().into_iter().find(|s| s.key == key).unwrap();
    assert_eq!(s.state, AutomationState::Armed);
    let next = s.next_fire.unwrap();
    assert!(next.starts_with("2026-10-29T09:00:"), "{next}");
    assert_eq!(s.next_check.as_deref(), Some("2026-10-29T08:00:00.000Z"));
    host.pause_watch(&key, true).unwrap();
    let s = host.list().into_iter().find(|s| s.key == key).unwrap();
    assert!(s.watch_paused_at.is_some());
    assert!(s.next_check.is_none());
}

#[test]
fn also_armed_in_pa_reads_pa_trust_read_only() {
    let fx = Fx::new();
    fx.manual("hello");
    fx.manual("other");
    let key = fx.user_key("hello");
    fx.pa_armed(std::slice::from_ref(&key));
    let pa_file = fx.pa.join("pi-automations").join("trust.json");
    let before = std::fs::read(&pa_file).unwrap();
    let mut host = fx.host();
    assert!(host.list().iter().all(|s| !s.also_armed_in_pa));
    host.arm(&key).unwrap();
    host.arm(&fx.user_key("other")).unwrap();
    let all = host.list();
    let badge = |name: &str| {
        all.iter()
            .find(|s| s.name == name)
            .unwrap()
            .also_armed_in_pa
    };
    assert!(badge("hello"));
    assert!(!badge("other"));
    assert_eq!(std::fs::read(&pa_file).unwrap(), before);
}

#[test]
fn preview_names_everything_consent_covers() {
    let fx = Fx::new();
    fx.write(
        &fx.repo_root(),
        "digest",
        "timeout: 10m\non:\n  schedule:\n    cron: \"0 8 * * MON-FRI\"\n    timezone: UTC\n  manual: true\nhooks:\n  pre:\n    - prompt: Get ready first.\n",
    );
    let key = fx.repo_key("digest");
    let mut host = fx.host();
    let p = host.preview(&key).unwrap();
    assert_eq!(p.name, "digest");
    assert!(p.triggers[0].starts_with("schedule: 0 8 * * MON-FRI (UTC), no catch-up — next "));
    assert_eq!(p.triggers[1], "manual: allowed");
    assert_eq!(p.timeout, "10m");
    assert_eq!(
        p.hooks,
        vec![(
            "pre 0".to_string(),
            "prompt: \"Get ready first.\"".to_string()
        )]
    );
    assert_eq!(p.files, vec!["AUTOMATION.md".to_string()]);
    assert!(p.expires.as_deref().unwrap().starts_with("in 30 days, on "));
    assert!(p.agent.is_none() && p.agent_command.is_none());
    assert!(
        p.warnings
            .iter()
            .any(|w| w == "no agent CLI chosen — `fm agent default <cli>`")
    );

    fx.config(&format!(
        "default-agent = \"claude\"\n[agents.claude]\npath = {}\nallow-all-tools = true\n",
        toml_path(&fm_exe())
    ));
    fx.pa_armed(std::slice::from_ref(&key));
    let p = host.preview(&key).unwrap();
    assert_eq!(p.agent, Some(CliId::Claude));
    let cmd = p.agent_command.unwrap();
    assert!(cmd.starts_with(&fm_exe().display().to_string()), "{cmd}");
    assert!(cmd.contains("--dangerously-skip-permissions"), "{cmd}");
    assert!(cmd.ends_with("< <prompt>"), "{cmd}");
    assert_eq!(
        p.warnings[0],
        "also armed in pi-automations — it would run twice; disarm it there with `pa disarm digest`"
    );
    assert!(!p.warnings.iter().any(|w| w.starts_with("no agent CLI")));

    // The agent command is what consent records.
    host.arm(&key).unwrap();
    let trust = fm_state::read_trust(host.paths());
    let rec = &trust.records[&key];
    assert!(
        rec.agent_command
            .as_deref()
            .unwrap()
            .contains("--dangerously-skip-permissions")
    );
    assert!(rec.expires_at.is_some());
    assert_eq!(rec.files.len(), 1);
}

#[test]
fn preview_of_an_invalid_automation_is_its_refusal() {
    let fx = Fx::new();
    fx.write(
        &fx.user_root(),
        "broken",
        "on:\n  schedule: \"61 * * * *\"\n",
    );
    let mut host = fx.host();
    let e = host.preview(&fx.user_key("broken")).unwrap_err();
    assert!(e.starts_with("broken: on.schedule"), "{e}");
    assert!(host.arm(&fx.user_key("broken")).is_err());
}

#[test]
fn runs_are_newest_first_and_live_reads_the_running_run() {
    let fx = Fx::new();
    fx.manual("hello");
    let key = fx.user_key("hello");
    let mut host = fx.host();
    assert!(host.runs(&key, 10).is_empty());
    assert!(host.live(&key).is_none());

    let paths = host.paths().clone();
    std::fs::create_dir_all(paths.locks_dir()).unwrap();
    let now = Utc::now();
    let lock = RunLockFile {
        info: own_lock_info(
            "20261029-100000-abcd",
            Trigger::Manual,
            Some("claude".into()),
            now,
            &SystemProbe,
        ),
        key: key.clone(),
        scheduled_for: None,
        budget_ms: None,
    };
    let Acquire::Acquired(held) = acquire_run_lock(&paths, &lock).unwrap() else {
        panic!("lock held");
    };
    let events = [
        LiveEvent::Session {
            id: "s1".into(),
            ts: 1,
        },
        LiveEvent::Tool {
            name: "bash".into(),
            summary: "ls".into(),
            ts: 2,
        },
    ];
    let text: String = events
        .iter()
        .map(|e| serde_json::to_string(e).unwrap() + "\n")
        .collect::<String>()
        + "not json\n";
    std::fs::write(paths.live_file(&key), text).unwrap();
    let live = host.live(&key).unwrap();
    assert_eq!(live.run_id, "20261029-100000-abcd");
    assert_eq!(live.events, events);
    let s = host.list().into_iter().find(|s| s.key == key).unwrap();
    assert_eq!(s.running.unwrap().run_id, "20261029-100000-abcd");

    // A stop is requested for exactly that run.
    assert!(host.stop(&key).unwrap().contains("stop requested"));
    assert!(fm_state::stop_requested(
        &paths,
        &key,
        "20261029-100000-abcd"
    ));
    assert!(host.run_now(&key).unwrap_err().contains("already running"));
    held.release();
    fm_state::clear_stop(&paths, &key);
    assert!(host.live(&key).is_none());
}

#[test]
fn set_agent_overrides_the_default() {
    let fx = Fx::new();
    fx.manual("hello");
    let key = fx.user_key("hello");
    let mut host = fx.host();
    host.set_default_agent(Some(CliId::Pi)).unwrap();
    assert_eq!(host.list()[0].agent.as_deref(), Some("pi"));
    host.set_agent(&key, Some(CliId::Codex)).unwrap();
    assert_eq!(host.list()[0].agent.as_deref(), Some("codex"));
    host.set_agent(&key, None).unwrap();
    assert_eq!(host.list()[0].agent.as_deref(), Some("pi"));
    assert_eq!(
        host.edit_path(&key),
        Some(fx.user_root().join("hello").join("AUTOMATION.md"))
    );
}

#[test]
fn agents_are_detected_once_per_session() {
    let fx = Fx::new();
    fx.config(&format!(
        "[agents.gemini]\npath = {}\n",
        toml_path(&fm_exe())
    ));
    let mut host = fx.host();
    let found = host.agents();
    assert_eq!(
        found,
        vec![(CliId::Gemini, Some(env!("CARGO_PKG_VERSION").to_string()))]
    );
    // Cached: a config change is not seen until the next session.
    fx.config("");
    assert_eq!(host.agents(), found);
}

#[test]
fn render_chart_draws_in_process() {
    // No fm on PATH and nothing configured: the chart is drawn by this binary itself.
    let fx = Fx::new();
    let mut host = fx.host();
    let chart = "flowchart TD\n  a[Start] --> b[Finish]\n";
    let progress = fm_model::RunProgress {
        chart: chart.into(),
        steps: vec![fm_model::RunStep {
            id: "a".into(),
            label: "Start".into(),
            status: fm_model::StepStatus::Done,
            text: None,
            started_at: None,
            ended_at: None,
            tasks: Vec::new(),
        }],
        taken: vec![],
        notes: vec![],
        message: None,
        history: Vec::new(),
    };
    let text = host.render_chart(chart, Some(&progress), 200).unwrap();
    assert!(
        text.contains("Start ✓") && text.contains("Finish"),
        "{text}"
    );
    // Too wide for the pane: the step list shows instead, with one line saying why.
    let e = host.render_chart(chart, Some(&progress), 3).unwrap_err();
    assert!(e.contains("columns wide") && !e.contains('\n'), "{e}");
    // A chart it cannot draw says so in one line.
    let e = host.render_chart("not a chart", None, 200).unwrap_err();
    assert!(e.starts_with("flowchart: ") && !e.contains('\n'), "{e}");
}

#[test]
fn watcher_status_from_the_last_tick() {
    let fx = Fx::new();
    let now = Utc.with_ymd_and_hms(2026, 10, 29, 8, 0, 0).unwrap();
    let mut host = fx.host().with_now(now);
    assert_eq!(host.watcher_status(), "off — fm watcher start");
    fm_state::StateStore::new(host.paths())
        .note_tick(
            now - chrono::Duration::seconds(12),
            fm_model::TickSource::Job,
        )
        .unwrap();
    assert_eq!(host.watcher_status(), "on — last tick 12s ago");
}
