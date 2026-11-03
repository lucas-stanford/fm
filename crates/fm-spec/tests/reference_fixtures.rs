//! The reference host's invalid fixtures (pi-automations test/helpers/fixtures.mjs): one
//! automation per refusal, each with the exact line it must produce.

use fm_spec::parse_automation;

/// (folder, expected refusal or `None` for a YAML-parser message, file)
const CASES: &[(&str, Option<&str>, &str)] = &[
    (
        "unknown-field",
        Some(
            r#"budget: unknown field — Draft 0.1.0 has name, description, enabled, model, timeout, on, hooks, flow"#,
        ),
        r#"---
name: unknown-field
description: Uses a field Draft 0.1.0 does not define.
budget: 10
on:
  manual: true
---

Body.
"#,
    ),
    (
        "unknown-fields",
        Some(
            r#"budget, retries: unknown fields — Draft 0.1.0 has name, description, enabled, model, timeout, on, hooks, flow"#,
        ),
        r#"---
name: unknown-fields
description: Uses two fields Draft 0.1.0 does not define.
retries: 3
budget: 10
on:
  manual: true
---

Body.
"#,
    ),
    (
        "unknown-trigger",
        Some(r#"on.webhook: this host implements schedule, manual, watch"#),
        r#"---
name: unknown-trigger
description: Uses a trigger type this host does not implement.
on:
  webhook: true
---

Body.
"#,
    ),
    (
        "bad-cron",
        Some(r#"on.schedule.cron: "0 7 * * MON-FRI *" is not a 5-field cron expression"#),
        r#"---
name: bad-cron
description: Has a six-field cron expression.
on:
  schedule:
    cron: "0 7 * * MON-FRI *"
---

Body.
"#,
    ),
    (
        "bad-tz",
        Some(r#"on.schedule.timezone: "PST" is not an IANA zone — try America/Los_Angeles"#),
        r#"---
name: bad-tz
description: Uses an abbreviation instead of an IANA zone.
on:
  schedule:
    cron: "0 7 * * *"
    timezone: PST
---

Body.
"#,
    ),
    (
        "bad-timeout",
        Some(r#"timeout: "ten minutes" is not a duration — use 90s, 10m, 1h"#),
        r#"---
name: bad-timeout
description: Spells the timeout out in words.
timeout: ten minutes
on:
  manual: true
---

Body.
"#,
    ),
    (
        "bad-hook-item",
        Some(r#"hooks.pre[1]: must be { script: <path> } or { prompt: <text> }"#),
        r#"---
name: bad-hook-item
description: Has a hook item that is neither a script nor a prompt.
on:
  manual: true
hooks:
  pre:
    - script: scripts/ok.js
    - command: ls
---

Body.
"#,
    ),
    (
        "bad-catch-up",
        Some(r#"on.schedule.catch-up: "later" is not skip or once"#),
        r#"---
name: bad-catch-up
description: Uses a catch-up value outside the two the spec defines.
on:
  schedule:
    cron: "0 7 * * *"
    catch-up: later
---

Body.
"#,
    ),
    (
        "bad-schedule-key",
        Some(r#"on.schedule.retries: unknown key — on.schedule has cron, timezone, catch-up"#),
        r#"---
name: bad-schedule-key
description: Adds a key to on.schedule that the spec does not define.
on:
  schedule:
    cron: "0 7 * * *"
    retries: 2
---

Body.
"#,
    ),
    (
        "name-mismatch",
        Some(
            r#"name: "daily-brief" does not match the directory name "name-mismatch" — they must be the same"#,
        ),
        r#"---
name: daily-brief
description: The name does not equal the directory name.
on:
  manual: true
---

Body.
"#,
    ),
    (
        "bad-name",
        Some(
            r#"name: "Daily_Briefing" is not a valid name — use lowercase letters, digits and single hyphens"#,
        ),
        r#"---
name: Daily_Briefing
description: The name uses characters the grammar does not allow.
on:
  manual: true
---

Body.
"#,
    ),
    (
        "bad-enabled",
        Some(r#"enabled: "yes" is not true or false — enabled is a boolean"#),
        r#"---
name: bad-enabled
description: Uses a string where a boolean belongs.
enabled: "yes"
on:
  manual: true
---

Body.
"#,
    ),
    (
        "bad-model",
        Some(
            r#"model: "claude-sonnet-4-5" is not <provider>/<id>[:<thinking>] — for example anthropic/claude-sonnet-4-5:low"#,
        ),
        r#"---
name: bad-model
description: Names a model without its provider.
model: claude-sonnet-4-5
on:
  manual: true
---

Body.
"#,
    ),
    (
        "no-trigger",
        Some(r#"on: needs at least one trigger — schedule, manual, watch"#),
        r#"---
name: no-trigger
description: Declares an empty trigger block.
on: {}
---

Body.
"#,
    ),
    (
        "missing-on",
        Some(r#"on: required — at least one trigger (schedule, manual, watch)"#),
        r#"---
name: missing-on
description: Has no trigger block at all.
---

Body.
"#,
    ),
    (
        "missing-description",
        Some(r#"description: required — ≤1024 chars saying what this automation does"#),
        r#"---
name: missing-description
on:
  manual: true
---

Body.
"#,
    ),
    (
        "no-frontmatter",
        Some(
            r#"frontmatter: missing — AUTOMATION.md starts with a --- line, then YAML, then another --- line"#,
        ),
        r#"# No frontmatter

Just a body.
"#,
    ),
    (
        "empty-body",
        Some(
            r#"body: missing — the Markdown body after the frontmatter is the prompt for each run"#,
        ),
        r#"---
name: empty-body
description: Has frontmatter but no body.
on:
  manual: true
---

"#,
    ),
    (
        "absolute-script",
        Some(
            r#"hooks.pre[0].script: "/etc/cron-helper.sh" is absolute — use a path relative to the automation directory"#,
        ),
        r#"---
name: absolute-script
description: Points a hook at an absolute path.
on:
  manual: true
hooks:
  pre:
    - script: /etc/cron-helper.sh
---

Body.
"#,
    ),
    (
        "escaping-script",
        Some(
            r#"hooks.pre[0].script: "../../other.js" leaves the automation directory — scripts must live inside it"#,
        ),
        r#"---
name: escaping-script
description: Points a hook outside its own directory.
on:
  manual: true
hooks:
  pre:
    - script: ../../other.js
---

Body.
"#,
    ),
    (
        "unquoted-cron",
        None,
        r#"---
name: unquoted-cron
description: Leaves a cron expression unquoted so YAML reads it as an alias.
on:
  schedule:
    cron: */15 * * * *
---

Body.
"#,
    ),
    (
        "duplicate-key",
        None,
        r#"---
name: duplicate-key
description: First description.
description: Second description.
on:
  manual: true
---

Body.
"#,
    ),
    (
        "bad-hooks-key",
        Some(r#"hooks.during: unknown key — hooks has pre, post"#),
        r#"---
name: bad-hooks-key
description: Adds a hook phase that does not exist.
on:
  manual: true
hooks:
  during:
    - prompt: nope
---

Body.
"#,
    ),
    (
        "bad-manual",
        Some(
            r#"on.manual: false is not true — write "manual: true", or remove it to disallow manual runs"#,
        ),
        r#"---
name: bad-manual
description: Writes manual false instead of removing it.
on:
  schedule: "0 7 * * *"
  manual: false
---

Body.
"#,
    ),
    (
        "watch-not-a-map",
        Some(
            r#"on.watch: must be { script: <path>, every: <duration> } — for example script: scripts/watch.js and every: 5m"#,
        ),
        r#"---
name: watch-not-a-map
description: Gives the watch a bare script path.
on:
  watch: scripts/watch.js
---

Body.
"#,
    ),
    (
        "bad-watch-key",
        Some(r#"on.watch.interval: unknown key — on.watch has script, every"#),
        r#"---
name: bad-watch-key
description: Spells the interval key its own way.
on:
  watch:
    script: scripts/watch.js
    interval: 5m
---

Body.
"#,
    ),
    (
        "watch-missing-script",
        Some(
            r#"on.watch.script: required — the script that looks for work, relative to the automation directory"#,
        ),
        r#"---
name: watch-missing-script
description: Has an interval and nothing to run.
on:
  watch:
    every: 5m
---

Body.
"#,
    ),
    (
        "escaping-watch-script",
        Some(
            r#"on.watch.script: "../watch.js" leaves the automation directory — scripts must live inside it"#,
        ),
        r#"---
name: escaping-watch-script
description: Points the watch outside its own directory.
on:
  watch:
    script: ../watch.js
    every: 5m
---

Body.
"#,
    ),
    (
        "watch-missing-every",
        Some(r#"on.watch.every: required — how often to run the script, for example 5m"#),
        r#"---
name: watch-missing-every
description: Has a script and no interval.
on:
  watch:
    script: scripts/watch.js
---

Body.
"#,
    ),
    (
        "bad-watch-every",
        Some(r#"on.watch.every: "5 minutes" is not a duration — use 1m, 5m, 1h"#),
        r#"---
name: bad-watch-every
description: Writes the interval in words.
on:
  watch:
    script: scripts/watch.js
    every: 5 minutes
---

Body.
"#,
    ),
    (
        "watch-too-often",
        Some(r#"on.watch.every: 30s is shorter than a minute — use 1m or more"#),
        r#"---
name: watch-too-often
description: Asks for a check more often than anything ticks.
on:
  watch:
    script: scripts/watch.js
    every: 30s
---

Body.
"#,
    ),
    (
        "watch-too-rare",
        Some(r#"on.watch.every: 25h is longer than 24h — use on.schedule for anything rarer"#),
        r#"---
name: watch-too-rare
description: Asks for a check less than once a day.
on:
  watch:
    script: scripts/watch.js
    every: 25h
---

Body.
"#,
    ),
];

#[test]
fn every_reference_fixture_is_refused_with_its_line() {
    assert!(CASES.len() >= 30);
    for (dir, expected, file) in CASES {
        let actual = parse_automation(file, dir)
            .map(|_| ())
            .expect_err(dir)
            .to_string();
        assert!(!actual.contains('\n'), "{dir}: {actual}");
        match expected {
            Some(e) => assert_eq!(actual, *e, "{dir}"),
            None => assert!(actual.starts_with("frontmatter: "), "{dir}: {actual}"),
        }
    }
}

#[test]
fn the_yaml_cases_name_their_cause() {
    let find = |dir: &str| CASES.iter().find(|c| c.0 == dir).unwrap();
    let (dir, _, file) = find("unquoted-cron");
    let cron = parse_automation(file, dir).unwrap_err().to_string();
    assert!(cron.ends_with(" — quote the cron expression"), "{cron}");
    let (dir, _, file) = find("duplicate-key");
    let dup = parse_automation(file, dir).unwrap_err().to_string();
    assert!(dup.to_lowercase().contains("unique"), "{dup}");
}
