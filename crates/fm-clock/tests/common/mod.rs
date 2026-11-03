#![allow(dead_code)]
//! A state folder and a home directory in a temporary folder, automations written and armed in it,
//! and fakes for starting runs and running scripts.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use fm_clock::{RunScript, StartRun, TickContext};
use fm_model::{Located, StatePaths, TickSource, Trigger};
use fm_run::process::{Output, ProcessOutcome, ProcessSpec};
use fm_spec::{DiscoverContext, discover, located, user_root};
use fm_state::{ArmShown, Consent, FakeProbe, Pin};

pub fn at(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

pub struct World {
    pub tmp: tempfile::TempDir,
    pub paths: StatePaths,
    pub home: PathBuf,
    pub probe: FakeProbe,
}

impl World {
    pub fn new() -> World {
        let tmp = tempfile::tempdir().unwrap();
        let paths = StatePaths::new(tmp.path().join("state"));
        fm_state::open_state(&paths).unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(automations_in(&home)).unwrap();
        World {
            tmp,
            paths,
            home,
            probe: FakeProbe::new().alive(std::process::id(), None),
        }
    }

    pub fn root(&self) -> PathBuf {
        automations_in(&self.home)
    }

    /// Write `<root>/<name>/AUTOMATION.md` with `on` (YAML lines under `on:`) and extra files.
    pub fn write(&self, name: &str, on: &str, files: &[(&str, &str)]) -> PathBuf {
        write_automation(&self.root(), name, on, "", files)
    }

    pub fn located(&self, name: &str) -> Located {
        located_in(&[user_root(&self.home)], name)
    }

    /// Arm `name` exactly as it is on disk; its key.
    pub fn arm(&self, name: &str, now: DateTime<Utc>) -> String {
        let l = self.located(name);
        arm(&self.paths, &l, now);
        l.key
    }

    pub fn ctx<'a>(
        &'a self,
        starter: &'a dyn StartRun,
        scripts: &'a dyn RunScript,
    ) -> TickContext<'a> {
        let mut ctx = TickContext::new(
            self.paths.clone(),
            self.home.clone(),
            TickSource::Job,
            starter,
        );
        ctx.probe = &self.probe;
        ctx.scripts = scripts;
        ctx.which = &fake_which;
        ctx
    }

    pub fn state(&self) -> fm_model::StateFile {
        fm_state::StateStore::new(&self.paths).load()
    }

    pub fn seed_last_tick(&self, iso: &str) {
        fm_state::StateStore::new(&self.paths)
            .update(|s| s.last_tick_at = Some(iso.to_string()))
            .unwrap();
    }

    pub fn records(&self, key: &str) -> Vec<fm_model::RunRecord> {
        fm_state::read_records(&self.paths, key, 100)
    }
}

pub fn automations_in(dir: &Path) -> PathBuf {
    dir.join(".agents").join("automations")
}

pub fn write_automation(
    root: &Path,
    name: &str,
    on: &str,
    extra: &str,
    files: &[(&str, &str)],
) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("AUTOMATION.md"),
        format!(
            "---\nname: {name}\ndescription: A test automation.\n{extra}on:\n{on}---\n\nDo the thing.\n"
        ),
    )
    .unwrap();
    for (rel, body) in files {
        let p = rel.split('/').fold(dir.clone(), |acc, part| acc.join(part));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
    }
    dir
}

pub fn located_in(roots: &[fm_model::Root], name: &str) -> Located {
    let found = discover(roots, &DiscoverContext::default());
    located(&found)
        .into_iter()
        .find(|l| l.automation.name == name)
        .unwrap_or_else(|| panic!("{name} did not parse: {found:?}"))
        .clone()
}

pub fn arm(paths: &StatePaths, l: &Located, now: DateTime<Utc>) {
    let d = fm_spec::digest_of(l).unwrap();
    let pin = Pin {
        digest: d.digest,
        files: d.files,
    };
    Consent::load(paths)
        .unwrap()
        .arm(l, &pin, &ArmShown::default(), now, 0)
        .unwrap();
}

/// Every interpreter is found (and never started: scripts run through [`Scripts`]).
pub fn fake_which(name: &str) -> Option<PathBuf> {
    Some(PathBuf::from("fake").join("bin").join(name))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    pub key: String,
    pub trigger: Trigger,
    pub scheduled_for: Option<String>,
    pub late: bool,
}

type OnStart<'w> = Box<dyn Fn(&Started) + 'w>;

/// Records every start; answers `result`. `on_start` sees the world at the moment of the start.
pub struct Starter<'w> {
    pub calls: RefCell<Vec<Started>>,
    pub result: Result<(), String>,
    pub on_start: Option<OnStart<'w>>,
}

impl Starter<'_> {
    pub fn ok() -> Self {
        Starter {
            calls: RefCell::new(Vec::new()),
            result: Ok(()),
            on_start: None,
        }
    }
    pub fn failing(reason: &str) -> Self {
        Starter {
            result: Err(reason.to_string()),
            ..Starter::ok()
        }
    }
    pub fn calls(&self) -> Vec<Started> {
        self.calls.borrow().clone()
    }
}

impl StartRun for Starter<'_> {
    fn start(
        &self,
        key: &str,
        trigger: Trigger,
        scheduled_for: Option<String>,
        late: bool,
    ) -> Result<(), String> {
        let s = Started {
            key: key.to_string(),
            trigger,
            scheduled_for,
            late,
        };
        if let Some(f) = &self.on_start {
            f(&s);
        }
        self.calls.borrow_mut().push(s);
        self.result.clone()
    }
}

type Answer<'w> = Box<dyn Fn(&ProcessSpec) -> ProcessOutcome + Sync + 'w>;

/// Answers every script without starting it, and keeps the specs.
pub struct Scripts<'w> {
    pub specs: Mutex<Vec<ProcessSpec>>,
    pub answer: Answer<'w>,
}

impl<'w> Scripts<'w> {
    pub fn new(answer: impl Fn(&ProcessSpec) -> ProcessOutcome + Sync + 'w) -> Self {
        Scripts {
            specs: Mutex::new(Vec::new()),
            answer: Box::new(answer),
        }
    }
    pub fn count(&self) -> usize {
        self.specs.lock().unwrap().len()
    }
    pub fn specs(&self) -> Vec<ProcessSpec> {
        self.specs.lock().unwrap().clone()
    }
}

impl RunScript for Scripts<'_> {
    fn run(&self, spec: &ProcessSpec) -> ProcessOutcome {
        self.specs.lock().unwrap().push(spec.clone());
        (self.answer)(spec)
    }
}

pub fn exited(exit: i32, stdout: &str, stderr: &str) -> ProcessOutcome {
    ProcessOutcome::Exited {
        exit,
        output: Output {
            stdout: stdout.into(),
            stderr: stderr.into(),
            output_bytes: stdout.len() as u64,
            truncated: false,
        },
    }
}

pub fn no_scripts() -> Scripts<'static> {
    Scripts::new(|_| panic!("no watch script should run"))
}
