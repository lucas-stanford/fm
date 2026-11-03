//! The state of each discovered automation, decided in the specification's order: `invalid`, then
//! `disabled`, then `off` (consent), then the machine checks for what would otherwise be `armed`.

use chrono::{DateTime, Utc};
use fm_model::{AutomationState, Discovered, Located, Scope};
use fm_spec::{Machine, check_machine, digest_of};
use fm_state::{Consent, Pin};

/// One discovered automation and its state.
#[derive(Debug, Clone)]
pub struct Assessed {
    pub key: String,
    pub state: AutomationState,
    /// For `invalid` and `off`: one line saying why.
    pub reason: Option<String>,
    /// The automation, when it parsed.
    pub located: Option<Located>,
}

impl Assessed {
    pub fn is_armed(&self) -> bool {
        self.state == AutomationState::Armed
    }
}

/// Decide the state of `d`. `machine` runs the machine checks (scripts, timeout maximum, model)
/// only for an automation that would otherwise be armed; `None` skips them.
pub fn assess(
    d: &Discovered,
    consent: &Consent,
    machine: Option<&Machine<'_>>,
    now: DateTime<Utc>,
) -> Assessed {
    let l = match d {
        Discovered::Invalid { key, error, .. } => {
            return Assessed {
                key: key.clone(),
                state: AutomationState::Invalid,
                reason: Some(error.clone()),
                located: None,
            };
        }
        Discovered::Valid(l) => l.as_ref(),
    };
    let out = |state, reason| Assessed {
        key: l.key.clone(),
        state,
        reason,
        located: Some(l.clone()),
    };
    if !l.automation.enabled {
        return out(AutomationState::Disabled, None);
    }
    let digest = match digest_of(l) {
        Ok(d) => d,
        Err(r) => return out(AutomationState::Invalid, Some(r.to_string())),
    };
    let pin = Pin {
        digest: digest.digest,
        files: digest.files,
    };
    if let Some(off) = consent.off_reason(l, &pin, now) {
        return out(AutomationState::Off, Some(off.message(&l.automation.name)));
    }
    if l.root.scope == Scope::Repository
        && let Some(repo) = l.repo_root.as_ref().filter(|r| !r.exists())
    {
        return out(
            AutomationState::Off,
            Some(format!("repository not found: {}", repo.display())),
        );
    }
    if let Some(m) = machine
        && let Err(r) = check_machine(l, m)
    {
        return out(AutomationState::Invalid, Some(r.to_string()));
    }
    out(AutomationState::Armed, None)
}
