//! A desktop notification when a run fails. Best effort: it never blocks a run for long, never
//! fails one, and is skipped when `FM_NO_NOTIFY` is set (the test suite sets it, so tests never
//! raise real notifications).

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use fm_model::{Outcome, RunRecord};
use fm_run::Notify;

const WAIT: Duration = Duration::from_secs(5);
const MAX_BODY: usize = 180;

/// Title and body, e.g. `daily-notes failed` / `claude exited 1: rate limited (after 2m)`.
pub fn notification_text(rec: &RunRecord) -> (String, String) {
    let said = rec
        .error
        .as_ref()
        .map(|e| e.message.clone())
        .or_else(|| rec.reason.clone())
        .unwrap_or_else(|| "failed".to_string());
    let said = said.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut body: String = said.chars().take(MAX_BODY).collect();
    if said.chars().count() > MAX_BODY {
        body.push('…');
    }
    let start = rec
        .started_at
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
    let end = rec
        .ended_at
        .as_deref()
        .and_then(|e| chrono::DateTime::parse_from_rfc3339(e).ok());
    if let (Some(s), Some(e)) = (start, end) {
        let secs = (e - s).num_seconds().max(0);
        body.push_str(&format!(" (after {})", short_duration(secs)));
    }
    (format!("{} failed", rec.name), body)
}

fn short_duration(secs: i64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s => format!("{}h {}m", s / 3600, (s % 3600) / 60),
    }
}

/// The command that shows a notification on this OS, or `None` where there is no way.
pub fn notify_command(title: &str, body: &str) -> Option<Command> {
    if cfg!(target_os = "macos") {
        let quote = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let mut c = Command::new("/usr/bin/osascript");
        c.arg("-e").arg(format!(
            "display notification \"{}\" with title \"fm\" subtitle \"{}\"",
            quote(body),
            quote(title)
        ));
        Some(c)
    } else if cfg!(windows) {
        // PowerShell's own app id: an unregistered one is silently dropped on current Windows.
        let quote = |s: &str| s.replace('\'', "''");
        let script = format!(
            "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null; \
             $t = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); \
             $n = $t.GetElementsByTagName('text'); \
             $n.Item(0).AppendChild($t.CreateTextNode('{}')) > $null; \
             $n.Item(1).AppendChild($t.CreateTextNode('{}')) > $null; \
             [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}}\\WindowsPowerShell\\v1.0\\powershell.exe').Show([Windows.UI.Notifications.ToastNotification]::new($t))",
            quote(title),
            quote(body)
        );
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let exe = std::path::Path::new(&root)
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        let mut c = Command::new(exe);
        c.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
        no_window(&mut c);
        Some(c)
    } else if cfg!(unix) {
        let mut c = Command::new("notify-send");
        c.args(["-a", "fm", title, body]);
        Some(c)
    } else {
        None
    }
}

#[cfg(windows)]
fn no_window(c: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    c.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn no_window(_c: &mut Command) {}

/// Show a notification for a failed record. Never panics, waits at most a few seconds.
pub fn notify_failed(rec: &RunRecord) {
    if rec.outcome != Outcome::Failed || std::env::var_os("FM_NO_NOTIFY").is_some() {
        return;
    }
    let (title, body) = notification_text(rec);
    let Some(mut cmd) = notify_command(&title, &body) else {
        return;
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let Ok(mut child) = cmd.spawn() else {
        return;
    };
    let start = Instant::now();
    while start.elapsed() < WAIT {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// The desktop notifier for `FireDeps::notify`.
pub fn desktop_notifier() -> Notify {
    Box::new(notify_failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fm_model::{ErrorKind, RunError, Scope, Trigger};

    fn rec() -> RunRecord {
        RunRecord {
            id: "20260101-090000-abcd".into(),
            key: "user:/x:daily".into(),
            name: "daily".into(),
            scope: Scope::User,
            root: "/x".into(),
            trigger: Trigger::Schedule,
            scheduled_for: None,
            late: false,
            recorded_at: "2026-01-01T09:02:00+00:00".into(),
            started_at: Some("2026-01-01T09:00:00+00:00".into()),
            ended_at: Some("2026-01-01T09:02:05+00:00".into()),
            outcome: Outcome::Failed,
            reason: None,
            error: Some(RunError {
                kind: ErrorKind::Agent,
                message: "claude exited 1:\nrate limited".into(),
            }),
            agent: Some("claude".into()),
            model: None,
            timeout: None,
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

    #[test]
    fn text_is_one_line_with_duration() {
        let (title, body) = notification_text(&rec());
        assert_eq!(title, "daily failed");
        assert!(!body.contains('\n'), "{body}");
        assert!(body.starts_with("claude exited 1:"), "{body}");
        assert!(body.contains("(after "), "{body}");
    }

    #[test]
    fn long_text_is_bounded() {
        let mut r = rec();
        r.error.as_mut().unwrap().message = "x".repeat(500);
        r.started_at = None;
        let (_, body) = notification_text(&r);
        assert_eq!(body.chars().count(), MAX_BODY + 1);
    }

    #[test]
    fn quotes_cannot_break_out() {
        let Some(c) = notify_command("it's \"x\"", "a'b") else {
            return;
        };
        let args: Vec<String> = c
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let all = args.join(" ");
        if cfg!(windows) {
            assert!(all.contains("'it''s \"x\"'"), "{all}");
            assert!(all.contains("'a''b'"), "{all}");
        } else if cfg!(target_os = "macos") {
            assert!(all.contains("subtitle \"it's \\\"x\\\"\""), "{all}");
        }
    }

    #[test]
    fn only_failed_records_notify() {
        // An ok record returns before building any command, even without FM_NO_NOTIFY.
        let mut r = rec();
        r.outcome = Outcome::Ok;
        notify_failed(&r);
    }
}
