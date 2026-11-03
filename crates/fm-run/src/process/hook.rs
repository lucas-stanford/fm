//! What a script hook's process outcome means for the run record.

use std::time::Duration;

use fm_model::{HookKind, HookResult, HookStatus, Phase};

use super::ProcessOutcome;

/// `45s`, `9m12s`, `1h`: how pi-automations writes a duration in a reason.
pub fn format_duration(d: Duration) -> String {
    let total = (d.as_millis() + 500) / 1000;
    if total == 0 {
        return "0s".into();
    }
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    let mut out = String::new();
    if h > 0 {
        out.push_str(&format!("{h}h"));
    }
    if m > 0 {
        out.push_str(&format!("{m}m"));
    }
    if s > 0 {
        out.push_str(&format!("{s}s"));
    }
    out
}

fn phase_str(p: Phase) -> &'static str {
    match p {
        Phase::Pre => "pre",
        Phase::Post => "post",
    }
}

/// The record of one script hook, as pi-automations' `runScriptHook` reads its executor:
///
/// | Outcome | Status | Reason |
/// |---|---|---|
/// | exit 0 | `ok` | |
/// | exit n, pre hook | `skipped` (ends the pre hooks; the run is `skipped`) | `pre[i] <source> exited n: <last stderr line>` |
/// | exit n, post hook | `failed` | `post[i] <source> exited n: <last stderr line>` |
/// | start failure | `failed` | `<source> could not start — <message>` |
/// | timeout | `failed` | `hook timeout after <duration>` |
/// | stop file | `failed` | `<phase>[i] <source> was stopped` |
/// | signal (Unix) | `failed` | `<phase>[i] <source> was killed by <signal>` |
///
/// A signal is a failure here, where pi-automations reads it as exit 0: a script killed from
/// outside did not finish its work. `output_bytes` and `truncated` (only when true) are set for a
/// process that ran.
pub fn to_hook_result(
    phase: Phase,
    index: usize,
    kind: HookKind,
    source: &str,
    outcome: &ProcessOutcome,
    duration: Duration,
) -> HookResult {
    let mut r = HookResult {
        phase,
        index,
        kind,
        source: source.to_string(),
        status: HookStatus::Failed,
        exit: None,
        duration_ms: u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
        reason: None,
        output_bytes: None,
        truncated: None,
    };
    if let Some(out) = outcome.output() {
        r.output_bytes = Some(out.output_bytes);
        r.truncated = out.truncated.then_some(true);
    }
    let at = format!("{}[{index}] {source}", phase_str(phase));
    match outcome {
        ProcessOutcome::Exited { exit: 0, .. } => {
            r.status = HookStatus::Ok;
            r.exit = Some(0);
        }
        ProcessOutcome::Exited { exit, output } => {
            r.status = match phase {
                Phase::Pre => HookStatus::Skipped,
                Phase::Post => HookStatus::Failed,
            };
            r.exit = Some(*exit);
            let last = output
                .stderr
                .trim()
                .lines()
                .map(|l| l.trim_end_matches('\r'))
                .rfind(|l| !l.is_empty());
            r.reason = Some(match last {
                Some(l) => format!("{at} exited {exit}: {l}"),
                None => format!("{at} exited {exit}"),
            });
        }
        ProcessOutcome::StartFailed { message } => {
            r.reason = Some(format!("{source} could not start — {message}"));
        }
        ProcessOutcome::TimedOut { after, .. } => {
            r.reason = Some(format!("hook timeout after {}", format_duration(*after)));
        }
        ProcessOutcome::Stopped { .. } => {
            r.reason = Some(format!("{at} was stopped"));
        }
        ProcessOutcome::Signaled { signal, .. } => {
            r.reason = Some(format!("{at} was killed by {signal}"));
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::super::Output;
    use super::*;

    fn out(stderr: &str, bytes: u64, truncated: bool) -> Output {
        Output {
            stdout: String::new(),
            stderr: stderr.into(),
            output_bytes: bytes,
            truncated,
        }
    }

    fn hook(phase: Phase, outcome: &ProcessOutcome) -> HookResult {
        to_hook_result(
            phase,
            0,
            HookKind::Script,
            "s.js",
            outcome,
            Duration::from_millis(1234),
        )
    }

    #[test]
    fn formats_durations_like_pi_automations() {
        assert_eq!(format_duration(Duration::from_millis(45_000)), "45s");
        assert_eq!(format_duration(Duration::from_millis(552_000)), "9m12s");
        assert_eq!(format_duration(Duration::from_secs(3600)), "1h");
        assert_eq!(format_duration(Duration::from_millis(300)), "0s");
        assert_eq!(format_duration(Duration::from_millis(1500)), "2s");
        assert_eq!(format_duration(Duration::from_secs(3661)), "1h1m1s");
    }

    #[test]
    fn exit_zero_is_ok_with_the_byte_count() {
        let r = hook(
            Phase::Pre,
            &ProcessOutcome::Exited {
                exit: 0,
                output: out("", 5, false),
            },
        );
        assert_eq!(r.status, HookStatus::Ok);
        assert_eq!(r.exit, Some(0));
        assert_eq!(r.output_bytes, Some(5));
        assert_eq!(r.truncated, None);
        assert_eq!(r.reason, None);
        assert_eq!(r.duration_ms, 1234);
        assert_eq!(r.source, "s.js");
    }

    #[test]
    fn keeps_the_pre_post_split_on_a_non_zero_exit() {
        let o = ProcessOutcome::Exited {
            exit: 3,
            output: out("noise\nnothing to do\n\n", 0, false),
        };
        let pre = hook(Phase::Pre, &o);
        assert_eq!(pre.status, HookStatus::Skipped);
        assert_eq!(pre.exit, Some(3));
        assert_eq!(
            pre.reason.as_deref(),
            Some("pre[0] s.js exited 3: nothing to do")
        );
        let post = hook(Phase::Post, &o);
        assert_eq!(post.status, HookStatus::Failed);
        assert_eq!(
            post.reason.as_deref(),
            Some("post[0] s.js exited 3: nothing to do")
        );

        let quiet = hook(
            Phase::Post,
            &ProcessOutcome::Exited {
                exit: 1,
                output: out("\r\n", 0, false),
            },
        );
        assert_eq!(quiet.reason.as_deref(), Some("post[0] s.js exited 1"));
    }

    #[test]
    fn a_timeout_is_a_failure_naming_the_limit() {
        let r = hook(
            Phase::Pre,
            &ProcessOutcome::TimedOut {
                after: Duration::from_secs(300),
                output: out("", 4, true),
            },
        );
        assert_eq!(r.status, HookStatus::Failed);
        assert_eq!(r.reason.as_deref(), Some("hook timeout after 5m"));
        assert_eq!(r.exit, None);
        // The truncation flag and the real byte count reach the record.
        assert_eq!(r.output_bytes, Some(4));
        assert_eq!(r.truncated, Some(true));
    }

    #[test]
    fn a_stop_is_its_own_failure_not_a_timeout() {
        let r = hook(
            Phase::Post,
            &ProcessOutcome::Stopped {
                output: out("", 0, false),
            },
        );
        assert_eq!(r.status, HookStatus::Failed);
        assert_eq!(r.reason.as_deref(), Some("post[0] s.js was stopped"));
    }

    #[test]
    fn a_start_failure_is_a_failure_with_the_message() {
        let r = hook(
            Phase::Pre,
            &ProcessOutcome::StartFailed {
                message: "no such file".into(),
            },
        );
        assert_eq!(r.status, HookStatus::Failed);
        assert_eq!(
            r.reason.as_deref(),
            Some("s.js could not start — no such file")
        );
        assert_eq!(r.output_bytes, None);
        assert_eq!(r.exit, None);
    }

    #[test]
    fn a_signal_is_a_failure() {
        let r = hook(
            Phase::Pre,
            &ProcessOutcome::Signaled {
                signal: "SIGKILL".into(),
                output: out("", 0, false),
            },
        );
        assert_eq!(r.status, HookStatus::Failed);
        assert_eq!(
            r.reason.as_deref(),
            Some("pre[0] s.js was killed by SIGKILL")
        );
    }
}
