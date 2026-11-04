//! What a run's record keeps of what happened: the answer, bounded, and what kind of failure an
//! agent's error is.

use fm_model::{ErrorKind, RunOutput};

/// How much of a run's answer the record keeps. The transcript is the complete copy.
pub const OUTPUT_MAX: usize = 2000;

/// What of the body's answer to keep; `None` when there is nothing.
pub fn clip_output(text: &str) -> Option<RunOutput> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.chars().count() <= OUTPUT_MAX {
        return Some(RunOutput {
            text: trimmed.to_string(),
            truncated: None,
        });
    }
    Some(RunOutput {
        text: trimmed.chars().take(OUTPUT_MAX).collect(),
        truncated: Some(true),
    })
}

const UNAVAILABLE: [&str; 18] = [
    "unauthor",
    "forbidden",
    "invalid api key",
    "no credentials",
    "authentication",
    "credential",
    "quota",
    "rate limit",
    "insufficient",
    "billing",
    "overload",
    "unavailable",
    "bad gateway",
    "service unavailable",
    "gateway timeout",
    "econnrefused",
    "enotfound",
    "eai_again",
];

/// What kind of failure an agent CLI's error is, by its words, as pi-automations'
/// `classifyFireError` reads an SDK error's message: the provider refusing or failing is
/// `model-unavailable`; anything else is `agent`.
pub fn classify_agent_error(message: &str) -> ErrorKind {
    let text = message.to_lowercase();
    if UNAVAILABLE.iter().any(|w| text.contains(w)) || text.contains("socket hang up") {
        ErrorKind::ModelUnavailable
    } else {
        ErrorKind::Agent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_answer_trimmed_and_bounded() {
        assert_eq!(clip_output("  \n "), None);
        let short = clip_output("\n done \n").unwrap();
        assert_eq!(short.text, "done");
        assert_eq!(short.truncated, None);
        let long = clip_output(&"é".repeat(OUTPUT_MAX + 5)).unwrap();
        assert_eq!(long.text.chars().count(), OUTPUT_MAX);
        assert_eq!(long.truncated, Some(true));
    }

    #[test]
    fn a_provider_refusing_is_model_unavailable() {
        for m in [
            "401 Unauthorized",
            "Invalid API key provided",
            "You exceeded your current quota",
            "Rate limit reached",
            "model is overloaded",
            "503 Service Unavailable",
            "connect ECONNREFUSED 127.0.0.1:443",
            "Not logged in · Please run /login (authentication required)",
        ] {
            assert_eq!(classify_agent_error(m), ErrorKind::ModelUnavailable, "{m}");
        }
        for m in [
            "claude exited with code 1 and wrote nothing to stderr",
            "tool call failed",
        ] {
            assert_eq!(classify_agent_error(m), ErrorKind::Agent, "{m}");
        }
    }
}
