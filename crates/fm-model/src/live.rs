//! What a run shows while it is happening. Written as JSON lines to the run's live file, read by
//! the run page. A live view nobody can write is a view nobody sees, never a failed run.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum LiveEvent {
    /// The agent session started; `id` is what resumes it.
    Session { id: String, ts: u64 },
    /// Reply text still arriving. Bounded, throttled.
    Partial { text: String, ts: u64 },
    /// Reasoning still arriving.
    Thinking { text: String, ts: u64 },
    /// A tool call as it starts.
    Tool {
        name: String,
        /// One line describing the arguments, bounded.
        summary: String,
        ts: u64,
    },
    /// A finished assistant message.
    Message { text: String, ts: u64 },
    /// The turn's final answer.
    Final { text: String, ts: u64 },
    /// The CLI reported an error.
    Error { message: String, ts: u64 },
    /// A hook started or ended.
    Hook {
        phase: crate::Phase,
        index: usize,
        source: String,
        status: Option<crate::HookStatus>,
        ts: u64,
    },
}

impl LiveEvent {
    /// When it happened, in milliseconds since the Unix epoch.
    pub fn ts(&self) -> u64 {
        match self {
            LiveEvent::Session { ts, .. }
            | LiveEvent::Partial { ts, .. }
            | LiveEvent::Thinking { ts, .. }
            | LiveEvent::Tool { ts, .. }
            | LiveEvent::Message { ts, .. }
            | LiveEvent::Final { ts, .. }
            | LiveEvent::Error { ts, .. }
            | LiveEvent::Hook { ts, .. } => *ts,
        }
    }
}

/// When `events[i]` is a `Final` that repeats word for word the last `Message` before it (the
/// usual case: the answer is the last thing the agent said), that message's index. A view shows
/// the answer once: `fm run` skips the `Final`, the run page drops the `Message`.
pub fn repeated_message(events: &[LiveEvent], i: usize) -> Option<usize> {
    let LiveEvent::Final { text, .. } = events.get(i)? else {
        return None;
    };
    let before = events.get(..i)?;
    let (j, said) = before.iter().enumerate().rev().find_map(|(j, e)| match e {
        LiveEvent::Message { text, .. } => Some((j, text)),
        _ => None,
    })?;
    (said == text).then_some(j)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(text: &str) -> LiveEvent {
        LiveEvent::Message {
            text: text.into(),
            ts: 1,
        }
    }

    fn answer(text: &str) -> LiveEvent {
        LiveEvent::Final {
            text: text.into(),
            ts: 2,
        }
    }

    #[test]
    fn a_final_repeating_the_last_message_points_at_it() {
        let session = LiveEvent::Session {
            id: "s".into(),
            ts: 2,
        };
        let events = [message("Paris."), session, answer("Paris.")];
        assert_eq!(repeated_message(&events, 2), Some(0));
        assert_eq!(repeated_message(&events, 0), None);
        assert_eq!(repeated_message(&events, 9), None);
    }

    #[test]
    fn a_final_that_says_something_new_repeats_nothing() {
        assert_eq!(repeated_message(&[answer("Paris.")], 0), None);
        let events = [message("Paris."), message("Lyon."), answer("Paris.")];
        assert_eq!(repeated_message(&events, 2), None);
    }
}
