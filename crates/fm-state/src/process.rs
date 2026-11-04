//! Is a process alive, and is it the same process that took a lock? Behind a trait so tests can
//! say which pids are alive without starting any.

use std::collections::BTreeMap;
use std::sync::Mutex;

use chrono::{DateTime, TimeZone, Utc};

use crate::time::parse_iso;

/// What the OS says about a pid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Gone,
    /// Running, with its start time when the OS tells it.
    Running {
        started_at: Option<DateTime<Utc>>,
    },
}

pub trait ProcessProbe: Send + Sync {
    fn probe(&self, pid: u32) -> ProcessState;
}

/// A start time this close to the recorded one is the same process: start times are whole seconds
/// on some systems and derived from boot time and ticks on others.
const SAME_START_SLACK_SECS: i64 = 2;

/// True when `pid` runs and, if the lock recorded a start time, it started then. A reused pid is
/// another process and so not the owner.
pub fn owner_alive(probe: &dyn ProcessProbe, pid: u32, recorded_start: Option<&str>) -> bool {
    if pid == 0 {
        return false;
    }
    match probe.probe(pid) {
        ProcessState::Gone => false,
        ProcessState::Running { started_at } => {
            match (recorded_start.and_then(parse_iso), started_at) {
                (Some(recorded), Some(actual)) => {
                    (recorded - actual).num_seconds().abs() <= SAME_START_SLACK_SECS
                }
                _ => true,
            }
        }
    }
}

/// The OS, through `sysinfo`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemProbe;

impl ProcessProbe for SystemProbe {
    fn probe(&self, pid: u32) -> ProcessState {
        use sysinfo::{Pid, ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System};
        if pid == 0 {
            return ProcessState::Gone;
        }
        let spid = Pid::from_u32(pid);
        let mut sys = System::new();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[spid]),
            true,
            ProcessRefreshKind::nothing(),
        );
        match sys.process(spid) {
            None => ProcessState::Gone,
            Some(p) if matches!(p.status(), ProcessStatus::Zombie | ProcessStatus::Dead) => {
                ProcessState::Gone
            }
            Some(p) => {
                let secs = i64::try_from(p.start_time()).unwrap_or(0);
                let started_at = if secs > 0 {
                    Utc.timestamp_opt(secs, 0).single()
                } else {
                    None
                };
                ProcessState::Running { started_at }
            }
        }
    }
}

/// Pids said to be alive, with their start times. Everything else is gone.
#[derive(Debug, Default)]
pub struct FakeProbe {
    alive: Mutex<BTreeMap<u32, Option<DateTime<Utc>>>>,
}

impl FakeProbe {
    pub fn new() -> Self {
        FakeProbe::default()
    }
    pub fn alive(self, pid: u32, started_at: Option<DateTime<Utc>>) -> Self {
        self.set_alive(pid, started_at);
        self
    }
    pub fn set_alive(&self, pid: u32, started_at: Option<DateTime<Utc>>) {
        self.alive
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(pid, started_at);
    }
    pub fn kill(&self, pid: u32) {
        self.alive
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&pid);
    }
}

impl ProcessProbe for FakeProbe {
    fn probe(&self, pid: u32) -> ProcessState {
        match self
            .alive
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&pid)
        {
            Some(started_at) => ProcessState::Running {
                started_at: *started_at,
            },
            None => ProcessState::Gone,
        }
    }
}

/// This process's start time, as the probe sees it.
pub fn own_start_time(probe: &dyn ProcessProbe) -> Option<DateTime<Utc>> {
    match probe.probe(std::process::id()) {
        ProcessState::Running { started_at } => started_at,
        ProcessState::Gone => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::iso;

    #[test]
    fn this_process_is_alive_and_nonsense_pids_are_not() {
        let me = std::process::id();
        assert!(matches!(
            SystemProbe.probe(me),
            ProcessState::Running { .. }
        ));
        assert_eq!(SystemProbe.probe(0), ProcessState::Gone);
        assert_eq!(SystemProbe.probe(1 << 30), ProcessState::Gone);
        assert!(owner_alive(&SystemProbe, me, None));
        assert!(!owner_alive(&SystemProbe, 0, None));
    }

    #[test]
    fn this_process_knows_when_it_started() {
        let started = own_start_time(&SystemProbe).expect("start time");
        assert!(started <= Utc::now());
        // And the recorded start time matches itself.
        assert!(owner_alive(
            &SystemProbe,
            std::process::id(),
            Some(&iso(started))
        ));
    }

    #[test]
    fn a_reused_pid_is_not_the_owner() {
        let t = Utc.with_ymd_and_hms(2026, 9, 17, 12, 0, 0).unwrap();
        let probe = FakeProbe::new().alive(42, Some(t));
        assert!(owner_alive(&probe, 42, Some(&iso(t))));
        assert!(owner_alive(
            &probe,
            42,
            Some(&iso(t + chrono::Duration::seconds(1)))
        ));
        assert!(!owner_alive(
            &probe,
            42,
            Some(&iso(t - chrono::Duration::hours(1)))
        ));
        // No recorded start time: alive is enough.
        assert!(owner_alive(&probe, 42, None));
        probe.kill(42);
        assert!(!owner_alive(&probe, 42, None));
    }
}
