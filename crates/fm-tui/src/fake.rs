//! A `Host` with realistic data in every state and section, for tests and `fm tui --fake` demos.
//!
//! Actions change its state the way a real host would, so a demo behaves: arming an automation
//! arms it, stopping a run records an aborted run, and so on. It never starts a process. Only
//! [`FakeHost::demo`] touches the filesystem: it writes sample `AUTOMATION.md` files to a
//! temporary folder so `e` has something to open, and notices when one changes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use fm_model::{
    ActionResult, Automation, AutomationState, CatchUp, CliId, ConsentPreview, Flow, Hook, Hooks,
    Host, LastRun, LiveEvent, LiveRun, ModelSpec, Outcome, Phase, RunError, RunLink, RunLockInfo,
    RunNote, RunOutput, RunProgress, RunRecord, RunStep, Schedule, Scope, Status, StepStatus,
    Trigger, Triggers, Watch, WatchState,
};

use crate::words::{Now, add_ms, parse, span_words};

const MIN: i64 = 60_000;
const HOUR: i64 = 60 * MIN;
const DAY: i64 = 24 * HOUR;

/// The flowchart `daily-briefing` is tracked on: a loop (urgent back to draft) and a shortcut
/// (gather straight to done).
pub const BRIEFING_CHART: &str = "flowchart TD
  gather[Gather inbox] --> triage[Triage threads]
  triage --> draft[Draft summary]
  draft --> review{Anything urgent?}
  review -->|yes| urgent[Flag urgent items]
  urgent --> draft
  review -->|no| post[Post to Slack]
  gather -.->|nothing new| done[Finish]
  post --> done";

/// A chart the fake cannot draw, so the run page falls back to its step list.
pub const TESTS_CHART: &str = "graph LR
  install[Install] --> test[Run tests] --> report[Report]";

const BRIEFING_NODES: [(&str, &str); 7] = [
    ("gather", "Gather inbox"),
    ("triage", "Triage threads"),
    ("draft", "Draft summary"),
    ("review", "Anything urgent?"),
    ("urgent", "Flag urgent items"),
    ("post", "Post to Slack"),
    ("done", "Finish"),
];

pub struct FakeHost {
    pub now: Now,
    statuses: Vec<Status>,
    /// Newest first.
    runs: BTreeMap<String, Vec<RunRecord>>,
    live: BTreeMap<String, LiveRun>,
    agents: Vec<(CliId, Option<String>)>,
    default_agent: CliId,
    watcher: String,
    /// Every call that changes something, `verb key [arg]`, for tests.
    pub calls: Vec<String>,
    demo_dir: Option<PathBuf>,
    demo_files: BTreeMap<String, (PathBuf, Option<SystemTime>)>,
    next_run: u32,
}

impl Default for FakeHost {
    fn default() -> Self {
        FakeHost::new()
    }
}

fn iso(t: &Now) -> String {
    t.to_rfc3339()
}

fn user_key(name: &str) -> String {
    format!("user:~/.agents/automations:{name}")
}

fn user_dir(name: &str) -> PathBuf {
    PathBuf::from("~")
        .join(".agents")
        .join("automations")
        .join(name)
}

fn model(provider: &str, id: &str) -> Option<ModelSpec> {
    Some(ModelSpec {
        provider: provider.into(),
        id: id.into(),
        thinking: None,
    })
}

fn automation(name: &str, description: &str, on: Triggers) -> Automation {
    Automation {
        name: name.into(),
        description: description.into(),
        enabled: true,
        model: model("anthropic", "claude-sonnet-5"),
        timeout: None,
        on,
        hooks: Hooks::default(),
        flow: Flow::Loose,
        body: format!("Do what {name} does."),
    }
}

fn schedule(cron: &str, tz: Option<&str>) -> Option<Schedule> {
    Some(Schedule {
        cron: cron.into(),
        timezone: tz.map(Into::into),
        catch_up: CatchUp::Once,
    })
}

fn watch(script: &str, every: &str) -> Option<Watch> {
    Some(Watch {
        script: script.into(),
        every: every.into(),
    })
}

fn status(
    name: &str,
    state: AutomationState,
    automation: Option<Automation>,
    agent: CliId,
) -> Status {
    Status {
        key: user_key(name),
        name: name.into(),
        scope: Scope::User,
        dir: user_dir(name),
        state,
        reason: None,
        problems: vec![],
        automation,
        agent: Some(agent.as_str().into()),
        effective_timeout_ms: 30 * MIN as u64,
        next_fire: None,
        next_check: None,
        watch: None,
        watch_paused_at: None,
        running: None,
        flow: false,
        last: None,
        also_armed_in_pa: false,
    }
}

fn link(label: &str, url: &str) -> RunLink {
    RunLink {
        label: Some(label.into()),
        url: url.into(),
    }
}

fn note(ts: &Now, text: &str, links: Vec<RunLink>) -> RunNote {
    RunNote {
        ts: iso(ts),
        text: text.into(),
        links,
    }
}

struct Rec<'a> {
    s: &'a Status,
    id: &'a str,
    trigger: Trigger,
    start: Now,
    took: i64,
    outcome: Outcome,
}

fn record(r: Rec<'_>) -> RunRecord {
    let end = add_ms(&r.start, r.took);
    let skipped = matches!(r.outcome, Outcome::Skipped | Outcome::Missed);
    RunRecord {
        id: r.id.into(),
        key: r.s.key.clone(),
        name: r.s.name.clone(),
        scope: r.s.scope,
        root: PathBuf::from("~").join(".agents").join("automations"),
        trigger: r.trigger,
        scheduled_for: (r.trigger == Trigger::Schedule).then(|| iso(&r.start)),
        late: false,
        recorded_at: iso(&end),
        started_at: (!skipped).then(|| iso(&r.start)),
        ended_at: (!skipped).then(|| iso(&end)),
        outcome: r.outcome,
        reason: None,
        error: None,
        agent: r.s.agent.clone(),
        model: Some("anthropic/claude-sonnet-5".into()),
        timeout: Some("30m".into()),
        hooks: vec![],
        output: None,
        session: None,
        chart: None,
        steps: vec![],
        taken: vec![],
        notes: vec![],
        message: None,
        count: None,
        pid: None,
    }
}

fn last_of(rec: &RunRecord) -> LastRun {
    LastRun {
        outcome: rec.outcome,
        at: rec
            .ended_at
            .clone()
            .unwrap_or_else(|| rec.recorded_at.clone()),
        reason: rec
            .error
            .as_ref()
            .map(|e| e.message.clone())
            .or_else(|| rec.reason.clone()),
        duration_ms: crate::words::took_ms(rec).map(|ms| ms as u64),
    }
}

fn briefing_steps(statuses: [StepStatus; 7], texts: [Option<&str>; 7]) -> Vec<RunStep> {
    BRIEFING_NODES
        .iter()
        .zip(statuses)
        .zip(texts)
        .map(|(((id, label), status), text)| RunStep {
            id: (*id).into(),
            label: (*label).into(),
            status,
            text: text.map(Into::into),
            started_at: None,
            ended_at: None,
        })
        .collect()
}

impl FakeHost {
    /// The fixed data every test uses: Thursday 2026-10-29, 08:30 at UTC−7.
    pub fn new() -> FakeHost {
        FakeHost::at(parse("2026-10-29T08:30:00-07:00").expect("fixed time"))
    }

    /// The same data around the real clock, with sample files `e` can open. For `fm tui --fake`.
    pub fn demo() -> FakeHost {
        let now = chrono::Local::now().fixed_offset();
        let mut host = FakeHost::at(now);
        host.demo_dir = Some(std::env::temp_dir().join("fm-tui-demo"));
        host
    }

    pub fn at(now: Now) -> FakeHost {
        let mut host = FakeHost {
            now,
            statuses: vec![],
            runs: BTreeMap::new(),
            live: BTreeMap::new(),
            agents: vec![
                (CliId::Claude, Some("2.1.4".into())),
                (CliId::Copilot, Some("1.0.102".into())),
                (CliId::Codex, Some("0.58.0".into())),
                (CliId::Pi, Some("0.71.2".into())),
                (CliId::Opencode, None),
            ],
            default_agent: CliId::Claude,
            watcher: "on — last tick 12s ago".into(),
            calls: vec![],
            demo_dir: None,
            demo_files: BTreeMap::new(),
            next_run: 1,
        };
        host.populate();
        host
    }

    fn t(&self, ms: i64) -> Now {
        add_ms(&self.now, ms)
    }

    fn populate(&mut self) {
        let now = self.now;
        let today = |h: u32, m: u32| {
            let d = now.date_naive().and_hms_opt(h, m, 0).expect("valid time");
            d.and_local_timezone(*now.offset())
                .single()
                .expect("fixed offset")
        };

        // Invalid, with its one-line refusal.
        let mut broken = status("broken-cron", AutomationState::Invalid, None, CliId::Claude);
        broken.reason = Some(
            "on.schedule.cron: \"0 9 * *\" has 4 fields — a cron is five fields: minute hour day month weekday"
                .into(),
        );
        broken.problems = vec![broken.reason.clone().unwrap_or_default()];

        // Armed with a schedule; its last run failed and needs you. Its chart cannot be drawn.
        let mut tests = status(
            "nightly-tests",
            AutomationState::Armed,
            Some(automation(
                "nightly-tests",
                "Run the whole test suite every night and report what broke.",
                Triggers {
                    schedule: schedule("0 2 * * *", None),
                    ..Default::default()
                },
            )),
            CliId::Codex,
        );
        tests.flow = true;
        tests.next_fire = Some(iso(&add_ms(&today(2, 0), DAY)));
        let mut failed = record(Rec {
            s: &tests,
            id: "20261029-020004-9c1e",
            trigger: Trigger::Schedule,
            start: today(2, 0),
            took: 3 * MIN + 12_000,
            outcome: Outcome::Failed,
        });
        failed.error = Some(RunError {
            kind: fm_model::ErrorKind::Agent,
            message: "codex exited 1: tests could not start — node_modules is missing".into(),
        });
        failed.chart = Some(TESTS_CHART.into());
        failed.steps = vec![
            RunStep {
                id: "install".into(),
                label: "Install".into(),
                status: StepStatus::Failed,
                text: Some("npm ci: ENOENT package-lock.json".into()),
                started_at: None,
                ended_at: None,
            },
            RunStep {
                id: "test".into(),
                label: "Run tests".into(),
                status: StepStatus::Skipped,
                text: None,
                started_at: None,
                ended_at: None,
            },
            RunStep {
                id: "report".into(),
                label: "Report".into(),
                status: StepStatus::Pending,
                text: None,
                started_at: None,
                ended_at: None,
            },
        ];
        failed.output = Some(RunOutput {
            text:
                "I could not start the tests: `npm ci` failed because package-lock.json is missing."
                    .into(),
            truncated: None,
        });
        let mut tests_ok = record(Rec {
            s: &tests,
            id: "20261028-020003-11ab",
            trigger: Trigger::Schedule,
            start: add_ms(&today(2, 0), -DAY),
            took: 11 * MIN,
            outcome: Outcome::Ok,
        });
        tests_ok.message = Some(note(
            &add_ms(&today(2, 11), -DAY),
            "All 1,284 tests passed.",
            vec![],
        ));
        tests.last = Some(last_of(&failed));
        self.runs.insert(tests.key.clone(), vec![failed, tests_ok]);

        // Armed with a watch whose last check failed.
        let mut inbox = status(
            "inbox-watch",
            AutomationState::Armed,
            Some(automation(
                "inbox-watch",
                "Read new mail and file it.",
                Triggers {
                    watch: watch("scripts/read_inbox.mjs", "5m"),
                    ..Default::default()
                },
            )),
            CliId::Claude,
        );
        inbox.next_check = Some(iso(&self.t(2 * MIN)));
        inbox.watch = Some(WatchState {
            checked_at: iso(&self.t(-3 * MIN)),
            exit: Some(2),
            problem: Some("scripts/read_inbox.mjs exited 2: IMAP login failed".into()),
            duration_ms: 1_850,
            ..Default::default()
        });

        // Off because it changed since it was armed; repository scope.
        let mut release = status(
            "release-notes",
            AutomationState::Off,
            Some(automation(
                "release-notes",
                "Draft release notes from merged pull requests.",
                Triggers {
                    manual: true,
                    ..Default::default()
                },
            )),
            CliId::Pi,
        );
        release.scope = Scope::Repository;
        release.key = "repository:~/src/app/.agents/automations:release-notes".into();
        release.dir = PathBuf::from("~")
            .join("src")
            .join("app")
            .join(".agents")
            .join("automations")
            .join("release-notes");
        release.reason = Some("changed since armed: scripts/collect.mjs".into());
        let rel = record(Rec {
            s: &release,
            id: "20261022-161200-77d0",
            trigger: Trigger::Manual,
            start: add_ms(&today(16, 12), -7 * DAY),
            took: 5 * MIN,
            outcome: Outcome::Ok,
        });
        release.last = Some(last_of(&rel));
        self.runs.insert(release.key.clone(), vec![rel]);

        // Running, tracked on a chart, with live events.
        let mut briefing = status(
            "daily-briefing",
            AutomationState::Armed,
            Some(automation(
                "daily-briefing",
                "Summarise my inbox and calendar every weekday morning and post it to Slack.",
                Triggers {
                    schedule: schedule("0 8 * * 1-5", Some("America/Los_Angeles")),
                    manual: true,
                    ..Default::default()
                },
            )),
            CliId::Claude,
        );
        briefing.flow = true;
        briefing.next_fire = Some(iso(&add_ms(&today(8, 0), DAY)));
        briefing.running = Some(RunLockInfo {
            run_id: "20261029-082800-5b7f".into(),
            pid: 4812,
            pid_started_at: None,
            started_at: iso(&self.t(-2 * MIN)),
            trigger: Trigger::Manual,
            agent: Some("claude".into()),
        });
        let history = self.briefing_history(&briefing, today(8, 0));
        briefing.last = history.first().map(last_of);
        self.runs.insert(briefing.key.clone(), history);
        self.live.insert(briefing.key.clone(), self.briefing_live());

        // Armed with a schedule, and armed in pi-automations too.
        let mut standup = status(
            "standup-notes",
            AutomationState::Armed,
            Some(automation(
                "standup-notes",
                "Write my standup notes from yesterday's commits.",
                Triggers {
                    schedule: schedule("0 9 * * 1-5", Some("America/Los_Angeles")),
                    ..Default::default()
                },
            )),
            CliId::Copilot,
        );
        standup.next_fire = Some(iso(&today(9, 0)));
        standup.also_armed_in_pa = true;
        let mut st = record(Rec {
            s: &standup,
            id: "20261028-090002-a1b2",
            trigger: Trigger::Schedule,
            start: add_ms(&today(9, 0), -DAY),
            took: 2 * MIN,
            outcome: Outcome::Ok,
        });
        st.message = Some(note(
            &add_ms(&today(9, 2), -DAY),
            "Yesterday: fixed the cron parser, reviewed #478. Today: release notes.",
            vec![link(
                "standup doc",
                "https://example.com/standup/2026-10-28",
            )],
        ));
        standup.last = Some(last_of(&st));
        self.runs.insert(standup.key.clone(), vec![st]);

        // Armed, by hand only.
        let mut branch = status(
            "branch-review",
            AutomationState::Armed,
            Some(automation(
                "branch-review",
                "Review the current branch before I open a pull request.",
                Triggers {
                    manual: true,
                    ..Default::default()
                },
            )),
            CliId::Pi,
        );
        let br = record(Rec {
            s: &branch,
            id: "20261026-143000-0f0f",
            trigger: Trigger::Manual,
            start: add_ms(&today(14, 30), -3 * DAY),
            took: 7 * MIN,
            outcome: Outcome::Ok,
        });
        branch.last = Some(last_of(&br));
        self.runs.insert(branch.key.clone(), vec![br]);

        // Armed with a watch; its last check said something.
        let mut prs = status(
            "pr-review",
            AutomationState::Armed,
            Some(automation(
                "pr-review",
                "Review new pull requests that ask for me.",
                Triggers {
                    watch: watch("scripts/new_prs.mjs", "10m"),
                    ..Default::default()
                },
            )),
            CliId::Codex,
        );
        prs.next_check = Some(iso(&self.t(7 * MIN)));
        prs.watch = Some(WatchState {
            checked_at: iso(&self.t(-3 * MIN)),
            answered_at: Some(iso(&self.t(-3 * MIN))),
            exit: Some(0),
            duration_ms: 640,
            said: Some("2 new pull requests: #481, #482".into()),
            fired_at: Some(iso(&self.t(-3 * MIN))),
            ..Default::default()
        });
        let mut pr = record(Rec {
            s: &prs,
            id: "20261029-082700-c3d4",
            trigger: Trigger::Watch,
            start: self.t(-3 * MIN),
            took: 2 * MIN + 30_000,
            outcome: Outcome::Ok,
        });
        pr.notes = vec![
            note(
                &self.t(-2 * MIN),
                "Reviewed #481: two comments on error handling",
                vec![link("#481", "https://github.com/acme/app/pull/481")],
            ),
            note(
                &self.t(-MIN),
                "Approved #482",
                vec![link("#482", "https://github.com/acme/app/pull/482")],
            ),
        ];
        prs.last = Some(last_of(&pr));
        self.runs.insert(prs.key.clone(), vec![pr]);

        // A paused watch.
        let mut docs = status(
            "docs-sync",
            AutomationState::Armed,
            Some(automation(
                "docs-sync",
                "Keep the docs site in step with the code.",
                Triggers {
                    watch: watch("scripts/changed_docs.sh", "1h"),
                    ..Default::default()
                },
            )),
            CliId::Copilot,
        );
        docs.watch_paused_at = Some(iso(&today(7, 45)));

        // Disabled.
        let mut cleanup = status(
            "weekly-cleanup",
            AutomationState::Disabled,
            Some(automation(
                "weekly-cleanup",
                "Close stale branches every Friday evening.",
                Triggers {
                    schedule: schedule("0 18 * * 5", None),
                    ..Default::default()
                },
            )),
            CliId::Claude,
        );
        if let Some(a) = cleanup.automation.as_mut() {
            a.enabled = false;
        }
        cleanup.reason = Some("enabled: false".into());

        // Off: nobody armed it yet. Hooks, so its preview shows them.
        let mut idea = status(
            "new-idea",
            AutomationState::Off,
            Some(automation(
                "new-idea",
                "Turn my notes into a weekly plan.",
                Triggers {
                    schedule: schedule("30 7 * * 1", None),
                    manual: true,
                    ..Default::default()
                },
            )),
            CliId::Claude,
        );
        if let Some(a) = idea.automation.as_mut() {
            a.hooks.pre = vec![Hook::Script("scripts/gather_notes.py".into())];
            a.hooks.post = vec![Hook::Prompt("Check the plan fits in one page.".into())];
            a.timeout = Some("20m".into());
        }
        idea.reason = Some("not armed".into());
        idea.effective_timeout_ms = 20 * MIN as u64;

        self.statuses = vec![
            broken, tests, inbox, release, briefing, standup, branch, prs, docs, cleanup, idea,
        ];
    }

    fn briefing_history(&self, s: &Status, eight: Now) -> Vec<RunRecord> {
        let mut yesterday = record(Rec {
            s,
            id: "20261028-080001-1a2b",
            trigger: Trigger::Schedule,
            start: add_ms(&eight, -DAY),
            took: 4 * MIN,
            outcome: Outcome::Ok,
        });
        yesterday.chart = Some(BRIEFING_CHART.into());
        yesterday.steps = briefing_steps(
            [
                StepStatus::Done,
                StepStatus::Done,
                StepStatus::Done,
                StepStatus::Done,
                StepStatus::Skipped,
                StepStatus::Done,
                StepStatus::Done,
            ],
            [
                Some("31 new threads"),
                None,
                None,
                Some("nothing urgent"),
                None,
                None,
                None,
            ],
        );
        yesterday.taken = vec![
            ("gather".into(), "triage".into()),
            ("triage".into(), "draft".into()),
            ("draft".into(), "review".into()),
            ("review".into(), "post".into()),
            ("post".into(), "done".into()),
        ];
        let at = add_ms(&eight, -DAY + 4 * MIN);
        yesterday.message = Some(note(
            &at,
            "Posted the briefing: 31 threads, 2 meetings moved, nothing urgent.",
            vec![link(
                "Slack thread",
                "https://acme.slack.com/archives/C01/p1730127840",
            )],
        ));
        yesterday.notes = vec![
            note(
                &at,
                "Summary posted to #team",
                vec![link(
                    "#team",
                    "https://acme.slack.com/archives/C01/p1730127840",
                )],
            ),
            note(
                &at,
                "Calendar: standup moved to 09:30",
                vec![link("event", "https://calendar.example.com/e/5521")],
            ),
        ];
        yesterday.output = Some(RunOutput {
            text: "Done. The briefing is in #team.".into(),
            truncated: None,
        });

        let mut tuesday = record(Rec {
            s,
            id: "20261027-080002-3c4d",
            trigger: Trigger::Schedule,
            start: add_ms(&eight, -2 * DAY),
            took: 6 * MIN,
            outcome: Outcome::Ok,
        });
        tuesday.message = Some(note(
            &add_ms(&eight, -2 * DAY + 6 * MIN),
            "Posted the briefing: 1 urgent thread flagged for you.",
            vec![],
        ));

        let mut monday = record(Rec {
            s,
            id: "20261026-080001-5e6f",
            trigger: Trigger::Schedule,
            start: add_ms(&eight, -3 * DAY),
            took: MIN + 5_000,
            outcome: Outcome::Failed,
        });
        monday.error = Some(RunError {
            kind: fm_model::ErrorKind::Agent,
            message: "claude exited 1: rate limited — try again later".into(),
        });

        let mut friday = record(Rec {
            s,
            id: "20261023-080000-7a8b",
            trigger: Trigger::Schedule,
            start: add_ms(&eight, -6 * DAY),
            took: 0,
            outcome: Outcome::Skipped,
        });
        friday.reason = Some("nothing new in the inbox".into());

        vec![yesterday, tuesday, monday, friday]
    }

    fn briefing_live(&self) -> LiveRun {
        let ts = |m: i64| self.t(m).timestamp_millis() as u64;
        LiveRun {
            run_id: "20261029-082800-5b7f".into(),
            events: vec![
                LiveEvent::Session {
                    id: "ses_8f2c41".into(),
                    ts: ts(-2 * MIN),
                },
                LiveEvent::Hook {
                    phase: Phase::Pre,
                    index: 0,
                    source: "scripts/fetch_inbox.mjs".into(),
                    status: Some(fm_model::HookStatus::Ok),
                    ts: ts(-2 * MIN + 4_000),
                },
                LiveEvent::Tool {
                    name: "read_file".into(),
                    summary: "inbox/2026-10-29.json".into(),
                    ts: ts(-100_000),
                },
                LiveEvent::Message {
                    text: "Found 14 unread threads and 3 calendar changes.".into(),
                    ts: ts(-80_000),
                },
                LiveEvent::Tool {
                    name: "bash".into(),
                    summary: "fm step draft -m \"reading 14 threads\"".into(),
                    ts: ts(-60_000),
                },
                LiveEvent::Thinking {
                    text: "Which of these need a reply today?".into(),
                    ts: ts(-30_000),
                },
                LiveEvent::Partial {
                    text: "Drafting the summary: 14 threads, grouped by project…".into(),
                    ts: ts(-5_000),
                },
            ],
            progress: Some(RunProgress {
                chart: BRIEFING_CHART.into(),
                steps: briefing_steps(
                    [
                        StepStatus::Done,
                        StepStatus::Done,
                        StepStatus::Active,
                        StepStatus::Pending,
                        StepStatus::Pending,
                        StepStatus::Pending,
                        StepStatus::Pending,
                    ],
                    [
                        Some("14 new threads"),
                        None,
                        Some("reading 14 threads"),
                        None,
                        None,
                        None,
                        None,
                    ],
                ),
                taken: vec![
                    ("gather".into(), "triage".into()),
                    ("triage".into(), "draft".into()),
                ],
                notes: vec![note(
                    &self.t(-MIN),
                    "Moved standup to 09:30",
                    vec![link("event", "https://calendar.example.com/e/5521")],
                )],
                message: None,
            }),
        }
    }

    fn find(&mut self, key: &str) -> Result<&mut Status, String> {
        self.statuses
            .iter_mut()
            .find(|s| s.key == key)
            .ok_or_else(|| format!("{key} is no longer listed"))
    }

    /// One automation's status, for tests.
    pub fn status(&self, key: &str) -> Option<&Status> {
        self.statuses.iter().find(|s| s.key == key)
    }

    /// The key of the automation called `name`.
    pub fn key_of(&self, name: &str) -> String {
        self.statuses
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.key.clone())
            .unwrap_or_default()
    }

    /// What a real host reports after a file of an armed automation changes.
    pub fn mark_changed(&mut self, key: &str, file: &str) {
        if let Some(s) = self
            .find(key)
            .ok()
            .filter(|s| s.state == AutomationState::Armed)
        {
            s.state = AutomationState::Off;
            s.reason = Some(format!("changed since armed: {file}"));
            s.next_fire = None;
        }
    }

    /// What a real host reports when an edit breaks the file.
    pub fn mark_invalid(&mut self, key: &str, reason: &str) {
        if let Ok(s) = self.find(key) {
            s.state = AutomationState::Invalid;
            s.reason = Some(reason.into());
            s.next_fire = None;
        }
    }

    pub fn set_watcher(&mut self, line: &str) {
        self.watcher = line.into();
    }

    /// One more event in the live view of a running automation.
    pub fn push_live(&mut self, key: &str, event: LiveEvent) {
        if let Some(l) = self.live.get_mut(key) {
            l.events.push(event);
        }
    }

    /// Only the automations named, for focused tests.
    pub fn keep_only(&mut self, names: &[&str]) {
        self.statuses.retain(|s| names.contains(&s.name.as_str()));
    }

    fn agent_command(&self, s: &Status) -> Option<String> {
        let cli: CliId = s.agent.as_deref()?.parse().ok()?;
        let model = s
            .automation
            .as_ref()
            .and_then(|a| a.model.as_ref())
            .map(|m| m.id.clone())
            .unwrap_or_else(|| "default".into());
        Some(match cli {
            CliId::Claude => format!(
                "claude -p --output-format stream-json --verbose --model {model} --permission-mode default"
            ),
            CliId::Copilot => {
                format!("copilot -p <prompt> --model {model} --no-color --log-level none")
            }
            CliId::Codex => {
                format!("codex exec --json --model {model} --sandbox workspace-write -")
            }
            CliId::Pi => format!("pi --mode json -p --model anthropic/{model} <prompt>"),
            CliId::Gemini => {
                format!("gemini -p <prompt> --model {model} --output-format stream-json")
            }
            CliId::Opencode => {
                format!("opencode run --format json --model anthropic/{model} <prompt>")
            }
        })
    }

    fn demo_file(&mut self, key: &str) -> Option<PathBuf> {
        let dir = self.demo_dir.clone()?;
        let s = self.statuses.iter().find(|s| s.key == key)?;
        let path = dir.join(&s.name).join("AUTOMATION.md");
        if !path.exists() {
            let a = s.automation.as_ref();
            let text = format!(
                "---\nname: {}\ndescription: {}\n---\n\n{}\n",
                s.name,
                a.map(|a| a.description.as_str())
                    .unwrap_or("A sample automation."),
                a.map(|a| a.body.as_str()).unwrap_or("Do the thing."),
            );
            std::fs::create_dir_all(path.parent()?).ok()?;
            std::fs::write(&path, text).ok()?;
        }
        let seen = modified(&path);
        self.demo_files
            .entry(key.to_string())
            .or_insert((path.clone(), seen));
        Some(path)
    }

    fn notice_demo_edits(&mut self) {
        let changed: Vec<String> = self
            .demo_files
            .iter()
            .filter(|(_, (path, seen))| modified(path) != *seen)
            .map(|(k, _)| k.clone())
            .collect();
        for key in changed {
            if let Some((path, seen)) = self.demo_files.get_mut(&key) {
                *seen = modified(path);
            }
            self.mark_changed(&key, "AUTOMATION.md");
        }
    }

    fn fake_chart(&self, progress: Option<&RunProgress>) -> Vec<String> {
        let status = |id: &str| {
            progress
                .and_then(|p| p.steps.iter().find(|s| s.id == id))
                .map_or(StepStatus::Pending, |s| s.status)
        };
        let g = |id: &str| crate::glyphs::Glyphs::UNICODE.step_list(status(id));
        // Pad `left` to the shortcut's column with `fill`, then close the line with `end`.
        let bar = |left: String, fill: char, end: &str| {
            let w = crate::text::width(&left);
            let pad: String = std::iter::repeat_n(fill, 52usize.saturating_sub(w)).collect();
            format!("{left}{pad}{end}")
        };
        vec![
            bar(format!("{} Gather inbox ", g("gather")), '─', "┐"),
            bar("│".into(), ' ', "│ nothing new"),
            bar(format!("{} Triage threads", g("triage")), ' ', "│"),
            bar("│".into(), ' ', "│"),
            bar(
                format!("{} Draft summary ◄─────────────┐", g("draft")),
                ' ',
                "│",
            ),
            bar(format!("│{}│", " ".repeat(29)), ' ', "│"),
            bar(
                format!(
                    "{} Anything urgent? ── yes ──► {} Flag urgent items",
                    g("review"),
                    g("urgent")
                ),
                ' ',
                "│",
            ),
            bar("│ no".into(), ' ', "│"),
            bar(format!("{} Post to Slack", g("post")), ' ', "│"),
            bar("│".into(), ' ', "│"),
            bar(format!("{} Finish ◄", g("done")), '─', "┘"),
        ]
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl Host for FakeHost {
    fn list(&mut self) -> Vec<Status> {
        self.notice_demo_edits();
        self.statuses.clone()
    }

    fn runs(&mut self, key: &str, limit: usize) -> Vec<RunRecord> {
        self.runs
            .get(key)
            .map(|r| r.iter().take(limit).cloned().collect())
            .unwrap_or_default()
    }

    fn live(&mut self, key: &str) -> Option<LiveRun> {
        let running = self.status(key)?.running.is_some();
        running.then(|| self.live.get(key).cloned()).flatten()
    }

    fn preview(&mut self, key: &str) -> Result<ConsentPreview, String> {
        let s = self.find(key)?.clone();
        let Some(a) = s.automation.clone() else {
            return Err(format!(
                "{}: {}",
                s.name,
                s.reason.clone().unwrap_or_else(|| "invalid".into())
            ));
        };
        let mut triggers = vec![];
        if let Some(sc) = &a.on.schedule {
            let tz = sc
                .timezone
                .as_deref()
                .map(|t| format!(" ({t})"))
                .unwrap_or_default();
            let catch = match sc.catch_up {
                CatchUp::Once => "catch-up once",
                CatchUp::Skip => "no catch-up",
            };
            triggers.push(format!("schedule: {}{tz}, {catch}", sc.cron));
        }
        if a.on.manual {
            triggers.push("manual: allowed".into());
        }
        let mut hooks = vec![];
        let mut files = vec!["AUTOMATION.md".to_string()];
        for (phase, list) in [("pre", &a.hooks.pre), ("post", &a.hooks.post)] {
            for (i, h) in list.iter().enumerate() {
                match h {
                    Hook::Script(p) => {
                        hooks.push((format!("{phase} {i}"), format!("script: python3 {p}")));
                        files.push(p.clone());
                    }
                    Hook::Prompt(p) => {
                        hooks.push((format!("{phase} {i}"), format!("prompt: \"{p}\"")))
                    }
                }
            }
        }
        let watch = a.on.watch.as_ref().map(|w| {
            files.push(w.script.clone());
            format!("node {} every {}", w.script, w.every)
        });
        let mut warnings = vec![];
        if s.also_armed_in_pa {
            warnings.push("also armed in pi-automations — it would run twice".into());
        }
        if s.state == AutomationState::Off && !crate::triage::never_armed(s.reason.as_deref()) {
            warnings.push(format!(
                "{} — what it runs is not what you armed",
                s.reason.clone().unwrap_or_default()
            ));
        }
        Ok(ConsentPreview {
            key: s.key.clone(),
            name: s.name.clone(),
            triggers,
            model: a
                .model
                .as_ref()
                .map(|m| m.display())
                .unwrap_or_else(|| "the agent's default".into()),
            timeout: span_words(s.effective_timeout_ms as i64),
            agent: s.agent.as_deref().and_then(|a| a.parse().ok()),
            agent_command: self.agent_command(&s),
            hooks,
            watch,
            files,
            warnings,
            expires: (s.scope == Scope::Repository).then(|| "in 30 days, on Nov 28".into()),
        })
    }

    fn arm(&mut self, key: &str) -> ActionResult {
        self.calls.push(format!("arm {key}"));
        let next = iso(&self.t(DAY));
        let s = self.find(key)?;
        match s.state {
            AutomationState::Invalid => {
                return Err(format!(
                    "{}: {}",
                    s.name,
                    s.reason.clone().unwrap_or_default()
                ));
            }
            AutomationState::Disabled => {
                return Err(format!("{} is disabled — enable it first", s.name));
            }
            AutomationState::Armed => return Ok(format!("{} is already armed", s.name)),
            AutomationState::Off => {}
        }
        s.state = AutomationState::Armed;
        s.reason = None;
        if s.automation
            .as_ref()
            .is_some_and(|a| a.on.schedule.is_some())
        {
            s.next_fire = Some(next);
        }
        Ok(format!("{} is armed", s.name))
    }

    fn disarm(&mut self, key: &str) -> ActionResult {
        self.calls.push(format!("disarm {key}"));
        let s = self.find(key)?;
        if s.state != AutomationState::Armed {
            return Err(format!("{} is not armed", s.name));
        }
        s.state = AutomationState::Off;
        s.reason = Some("not armed".into());
        s.next_fire = None;
        Ok(format!("{} is disarmed", s.name))
    }

    fn set_enabled(&mut self, key: &str, enabled: bool) -> ActionResult {
        self.calls.push(format!("set_enabled {key} {enabled}"));
        let next = iso(&self.t(DAY));
        let s = self.find(key)?;
        if s.state == AutomationState::Invalid {
            return Err(format!("{}: fix its file first", s.name));
        }
        if let Some(a) = s.automation.as_mut() {
            a.enabled = enabled;
        }
        if enabled {
            s.state = AutomationState::Armed;
            s.reason = None;
            if s.automation
                .as_ref()
                .is_some_and(|a| a.on.schedule.is_some())
            {
                s.next_fire = Some(next);
            }
            Ok(format!("{} is enabled", s.name))
        } else {
            s.state = AutomationState::Disabled;
            s.reason = Some("enabled: false".into());
            s.next_fire = None;
            Ok(format!("{} is disabled", s.name))
        }
    }

    fn run_now(&mut self, key: &str) -> ActionResult {
        self.calls.push(format!("run_now {key}"));
        let id = format!("20261029-fake-{:04}", self.next_run);
        self.next_run += 1;
        let started = iso(&self.now);
        let ts = self.now.timestamp_millis() as u64;
        let s = self.find(key)?;
        if s.running.is_some() {
            return Err(format!("{} is already running", s.name));
        }
        if s.state != AutomationState::Armed {
            return Err(format!("{} is not armed — arm it first", s.name));
        }
        s.running = Some(RunLockInfo {
            run_id: id.clone(),
            pid: 5000 + s.name.len() as u32,
            pid_started_at: None,
            started_at: started,
            trigger: Trigger::Manual,
            agent: s.agent.clone(),
        });
        let name = s.name.clone();
        self.live.insert(
            key.to_string(),
            LiveRun {
                run_id: id,
                events: vec![LiveEvent::Session {
                    id: "ses_new".into(),
                    ts,
                }],
                progress: None,
            },
        );
        Ok(format!("{name} started"))
    }

    fn stop(&mut self, key: &str) -> ActionResult {
        self.calls.push(format!("stop {key}"));
        let now = self.now;
        let s = self.find(key)?;
        let Some(lock) = s.running.take() else {
            return Err(format!("{} is not running", s.name));
        };
        let start = parse(&lock.started_at).unwrap_or(now);
        let mut rec = record(Rec {
            s,
            id: &lock.run_id,
            trigger: lock.trigger,
            start,
            took: (now - start).num_milliseconds(),
            outcome: Outcome::Aborted,
        });
        rec.reason = Some("stopped by you".into());
        s.last = Some(last_of(&rec));
        let name = s.name.clone();
        self.live.remove(key);
        self.runs.entry(key.to_string()).or_default().insert(0, rec);
        Ok(format!("{name} was stopped"))
    }

    fn pause_watch(&mut self, key: &str, paused: bool) -> ActionResult {
        self.calls.push(format!("pause_watch {key} {paused}"));
        let at = iso(&self.now);
        let s = self.find(key)?;
        if s.automation
            .as_ref()
            .and_then(|a| a.on.watch.as_ref())
            .is_none()
        {
            return Err(format!("{} has no watch", s.name));
        }
        s.watch_paused_at = paused.then_some(at);
        Ok(format!(
            "{}'s watch is {}",
            s.name,
            if paused { "paused" } else { "resumed" }
        ))
    }

    fn set_agent(&mut self, key: &str, agent: Option<CliId>) -> ActionResult {
        self.calls.push(format!(
            "set_agent {key} {}",
            agent.map_or("default", |a| a.as_str())
        ));
        let default = self.default_agent;
        let s = self.find(key)?;
        let cli = agent.unwrap_or(default);
        s.agent = Some(cli.as_str().into());
        Ok(match agent {
            Some(a) => format!("{} runs through {}", s.name, a.title()),
            None => format!("{} uses the default agent, {}", s.name, default.title()),
        })
    }

    fn agents(&mut self) -> Vec<(CliId, Option<String>)> {
        self.agents.clone()
    }

    fn edit_path(&mut self, key: &str) -> Option<PathBuf> {
        if self.demo_dir.is_some() {
            return self.demo_file(key);
        }
        self.status(key).map(|s| s.dir.join("AUTOMATION.md"))
    }

    fn render_chart(
        &mut self,
        chart: &str,
        progress: Option<&RunProgress>,
        width: u16,
    ) -> Result<String, String> {
        if chart.trim_start().starts_with("graph LR") {
            return Err("fm cannot draw a left-to-right chart yet".into());
        }
        let lines = self.fake_chart(progress);
        let need = lines
            .iter()
            .map(|l| crate::text::width(l))
            .max()
            .unwrap_or(0);
        if need > width as usize {
            return Err(format!(
                "the flowchart is {need} columns wide and the pane is {width}"
            ));
        }
        Ok(lines.join("\n"))
    }

    fn watcher_status(&mut self) -> String {
        self.watcher.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::triage::{Kind, classify};

    #[test]
    fn covers_every_state() {
        let mut h = FakeHost::new();
        let all = h.list();
        for state in [
            AutomationState::Invalid,
            AutomationState::Disabled,
            AutomationState::Off,
            AutomationState::Armed,
        ] {
            assert!(all.iter().any(|s| s.state == state), "{state:?}");
        }
        let kinds: Vec<Kind> = all.iter().map(|s| classify(s).1).collect();
        for k in [
            Kind::Invalid,
            Kind::Failed,
            Kind::WatchProblem,
            Kind::Consent,
            Kind::Running,
            Kind::Coming,
            Kind::Manual,
            Kind::Watched,
            Kind::Paused,
            Kind::Disabled,
            Kind::NotArmed,
        ] {
            assert!(kinds.contains(&k), "{k:?}");
        }
        assert!(all.iter().any(|s| s.also_armed_in_pa));
    }

    #[test]
    fn running_has_live_progress() {
        let mut h = FakeHost::new();
        let key = h.key_of("daily-briefing");
        let live = h.live(&key).expect("live");
        assert!(!live.events.is_empty());
        assert_eq!(crate::triage::step_of(Some(&live)), Some((3, 7)));
        assert!(h.live(&h.key_of("standup-notes")).is_none());
    }

    #[test]
    fn chart_renders_or_refuses() {
        let mut h = FakeHost::new();
        assert!(h.render_chart(BRIEFING_CHART, None, 80).is_ok());
        assert!(h.render_chart(BRIEFING_CHART, None, 20).is_err());
        assert!(h.render_chart(TESTS_CHART, None, 120).is_err());
    }

    #[test]
    fn actions_change_state() {
        let mut h = FakeHost::new();
        let idea = h.key_of("new-idea");
        assert!(h.arm(&idea).is_ok());
        assert_eq!(h.status(&idea).unwrap().state, AutomationState::Armed);
        assert!(h.run_now(&idea).is_ok());
        assert!(h.run_now(&idea).is_err());
        assert!(h.stop(&idea).is_ok());
        assert_eq!(h.runs(&idea, 5)[0].outcome, Outcome::Aborted);
        assert!(h.set_agent(&idea, Some(CliId::Codex)).is_ok());
        assert_eq!(h.status(&idea).unwrap().agent.as_deref(), Some("codex"));
        h.mark_changed(&idea, "AUTOMATION.md");
        assert_eq!(h.status(&idea).unwrap().state, AutomationState::Off);
    }

    #[test]
    fn preview_has_command_and_warnings() {
        let mut h = FakeHost::new();
        let p = h.preview(&h.key_of("standup-notes")).unwrap();
        assert!(p.agent_command.unwrap().starts_with("copilot "));
        assert_eq!(p.warnings.len(), 1);
        assert!(h.preview(&h.key_of("broken-cron")).is_err());
    }
}
