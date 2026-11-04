---
title: Validation messages
nav: Validation messages
description: Every message a host gives when it refuses an AUTOMATION.md, grouped by field, with a file that produces it.
order: 3
---

A host refuses a file it cannot honour with one line, `<field path>: <what is wrong> — <what fixes it>`, and reports only the first problem, in the order the [Specification](specification.md#validation) gives. This page lists every refusal of the file itself, by field, each with a file that produces it. A host keeps the shape and the field path; the rest of the wording is its own. The checks that need the machine are listed in the [Specification](specification.md#validation).

## Frontmatter and body

No frontmatter at all:

```markdown automation dir=no-frontmatter invalid="frontmatter: missing — AUTOMATION.md starts with a --- line, then YAML, then another --- line"
# Daily notes

Write today's notes.
```

YAML that does not parse. Duplicate keys are an error in YAML 1.2:

```yaml frontmatter invalid="frontmatter: Map keys must be unique at line 5, column 1:"
name: twice-on
description: Says when to run twice.
on:
  manual: true
on:
  schedule: "0 9 * * *"
```

An unquoted cron starting with `*` is read as a YAML alias:

```yaml frontmatter invalid="frontmatter: Unexpected alias at node end at line 5, column 16: — quote the cron expression"
name: every-quarter-hour
description: Runs every fifteen minutes.
on:
  schedule:
    cron: */15 * * * *
```

Frontmatter that is empty, or not a mapping:

```markdown automation dir=empty-frontmatter invalid="frontmatter: is empty — Draft 0.1.0 needs at least name, description and on"
---
---

Say hello.
```

```markdown automation dir=list-frontmatter invalid="frontmatter: must be a YAML mapping of fields"
---
- name: list-frontmatter
---

Say hello.
```

No body:

```markdown automation dir=empty-body invalid="body: missing — the Markdown body after the frontmatter is the prompt for each run"
---
name: empty-body
description: Has no body.
on:
  manual: true
---
```

## Unknown fields

Every unknown field is named, in one line:

```yaml frontmatter invalid="budget, schedule: unknown fields — Draft 0.1.0 has name, description, enabled, model, timeout, on, hooks, flow"
name: nightly-report
description: Post the nightly report.
schedule: "0 2 * * *"
budget: 5
on:
  manual: true
```

```yaml frontmatter invalid="tags: unknown field — Draft 0.1.0 has name, description, enabled, model, timeout, on, hooks, flow"
name: tagged-report
description: Post the tagged report.
tags: [reports]
on:
  manual: true
```

## `name`

```yaml frontmatter dir=missing-name invalid="name: required — ≤64 chars, lowercase letters, digits and hyphens, same as the directory name"
description: Has no name.
on:
  manual: true
```

```yaml frontmatter invalid="name: must be a string — lowercase letters, digits and hyphens"
name: 42
description: Has a number for a name.
on:
  manual: true
```

```yaml frontmatter invalid="name: is 68 characters — the maximum is 64"
name: a-very-long-automation-name-that-goes-on-and-on-past-the-limit-of-64
description: Has a name that is too long.
on:
  manual: true
```

```yaml frontmatter invalid="name: \"Daily_Report\" is not a valid name — use lowercase letters, digits and single hyphens"
name: Daily_Report
description: Post the daily report.
on:
  manual: true
```

```yaml frontmatter dir=daily-report-v2 invalid="name: \"daily-report\" does not match the directory name \"daily-report-v2\" — they must be the same"
name: daily-report
description: Post the daily report.
on:
  manual: true
```

## `description`

A description longer than 1024 characters is refused with `description: is <n> characters — the maximum is 1024`.

```yaml frontmatter invalid="description: required — ≤1024 chars saying what this automation does"
name: no-description
on:
  manual: true
```

```yaml frontmatter invalid="description: must be a string — ≤1024 chars saying what this automation does"
name: list-description
description: [one, two]
on:
  manual: true
```

## `enabled`

```yaml frontmatter invalid="enabled: \"no\" is not true or false — enabled is a boolean"
name: disabled-report
description: A report that is disabled for now.
enabled: no
on:
  manual: true
```

```yaml frontmatter invalid="enabled: \"false\" is not true or false — enabled is a boolean"
name: quoted-report
description: A report with a quoted switch.
enabled: "false"
on:
  manual: true
```

## `model`

```yaml frontmatter invalid="model: \"claude-sonnet\" is not <provider>/<id>[:<thinking>] — for example anthropic/claude-sonnet-4-5:low"
name: weekly-summary
description: Summarise the week.
model: claude-sonnet
on:
  manual: true
```

The id has no `/`:

```yaml frontmatter invalid="model: \"openrouter/meta-llama/llama-3\" is not <provider>/<id>[:<thinking>] — for example anthropic/claude-sonnet-4-5:low"
name: nested-model
description: Names a model id with a slash.
model: openrouter/meta-llama/llama-3
on:
  manual: true
```

## `timeout`

```yaml frontmatter invalid="timeout: \"1h30m\" is not a duration — use 90s, 10m, 1h"
name: long-audit
description: Audit every dependency.
timeout: 1h30m
on:
  manual: true
```

```yaml frontmatter invalid="timeout: 90 is not a duration — use 90s, 10m, 1h"
name: bare-timeout
description: Gives a timeout without a unit.
timeout: 90
on:
  manual: true
```

## `on`

```yaml frontmatter invalid="on: required — at least one trigger (schedule, manual, watch)"
name: no-trigger
description: Never says when to run.
```

```yaml frontmatter invalid="on: must be a mapping of trigger types (schedule, manual, watch)"
name: word-trigger
description: Names a trigger instead of mapping it.
on: manual
```

```yaml frontmatter invalid="on: needs at least one trigger — schedule, manual, watch"
name: empty-trigger
description: Has an empty trigger mapping.
on: {}
```

A trigger type the host does not implement. A host without `on.watch` refuses `on.watch` the same way:

```yaml frontmatter invalid="on.webhook: this host implements schedule, manual, watch"
name: deploy-notes
description: Write notes for each deploy.
on:
  webhook:
    path: /deploy
```

```yaml frontmatter invalid="on.manual: false is not true — write \"manual: true\", or remove it to disallow manual runs"
name: nightly-cleanup
description: Remove stale branches.
on:
  schedule: "0 2 * * *"
  manual: false
```

## `on.schedule`

```yaml frontmatter invalid="on.schedule: must be a cron string or a mapping of cron, timezone, catch-up"
name: listed-schedule
description: Gives the schedule as a list.
on:
  schedule: ["0 9 * * *"]
```

```yaml frontmatter invalid="on.schedule.tz: unknown key — on.schedule has cron, timezone, catch-up"
name: zoned-schedule
description: Uses a key on.schedule does not have.
on:
  schedule:
    cron: "0 9 * * *"
    tz: UTC
```

```yaml frontmatter invalid="on.schedule.cron: required — 5 fields, for example \"0 7 * * MON-FRI\""
name: zone-only
description: Has a timezone but no cron.
on:
  schedule:
    timezone: UTC
```

### `cron`

```yaml frontmatter invalid="on.schedule.cron: \"@daily\" is a nickname — Draft 0.1.0 takes 5 cron fields, so write \"0 0 * * *\""
name: nightly-digest
description: A nightly digest.
on:
  schedule: "@daily"
```

```yaml frontmatter invalid="on.schedule.cron: \"0 0 7 * * MON-FRI\" is not a 5-field cron expression"
name: with-seconds
description: Uses a six-field cron.
on:
  schedule: "0 0 7 * * MON-FRI"
```

```yaml frontmatter invalid="on.schedule.cron: \"0 9 L * *\" is not a valid cron expression — day-of-month \"L\" uses \"L\", which Draft 0.1.0 does not define — use *, a value, a range, a list or a step"
name: month-end-report
description: Report on the last day of the month.
on:
  schedule: "0 9 L * *"
```

```yaml frontmatter invalid="on.schedule.cron: \"0 25 * * *\" is not a valid cron expression — hour \"25\" is out of range 0-23"
name: late-night
description: Runs at an hour that does not exist.
on:
  schedule: "0 25 * * *"
```

```yaml frontmatter invalid="on.schedule.cron: \"0 9 * * FRI-MON\" is not a valid cron expression — day-of-week \"FRI-MON\" runs backwards — write it low to high"
name: weekend-wrap
description: Writes the range backwards.
on:
  schedule: "0 9 * * FRI-MON"
```

```yaml frontmatter invalid="on.schedule.cron: \"5/15 * * * *\" is not a valid cron expression — minute \"5/15\" applies a step to a single value — use */15 or a range like 0-30/15"
name: quarter-past
description: Steps from a single value.
on:
  schedule: "5/15 * * * *"
```

### `timezone` and `catch-up`

```yaml frontmatter invalid="on.schedule.timezone: \"PST\" is not an IANA zone — try America/Los_Angeles"
name: morning-check
description: Check the overnight build.
on:
  schedule:
    cron: "0 7 * * *"
    timezone: PST
```

```yaml frontmatter invalid="on.schedule.catch-up: \"always\" is not skip or once"
name: catch-everything
description: Wants every missed run.
on:
  schedule:
    cron: "0 7 * * *"
    catch-up: always
```

## `on.watch`

```yaml frontmatter invalid="on.watch: must be { script: <path>, every: <duration> } — for example script: scripts/watch.js and every: 5m"
name: shorthand-watch
description: Tries a shorthand watch.
on:
  watch: scripts/watch.js
```

```yaml frontmatter invalid="on.watch.interval: unknown key — on.watch has script, every"
name: interval-watch
description: Uses a key on.watch does not have.
on:
  watch:
    script: scripts/watch.js
    interval: 5m
```

```yaml frontmatter invalid="on.watch.script: required — the script that looks for work, relative to the automation directory"
name: scriptless-watch
description: Watches without a script.
on:
  watch:
    every: 5m
```

```yaml frontmatter invalid="on.watch.script: \"/usr/local/bin/watch.sh\" is absolute — use a path relative to the automation directory"
name: absolute-watch
description: Watches with a script outside its folder.
on:
  watch:
    script: /usr/local/bin/watch.sh
    every: 5m
```

```yaml frontmatter invalid="on.watch.every: required — how often to run the script, for example 5m"
name: timeless-watch
description: Watches without saying how often.
on:
  watch:
    script: scripts/watch.js
```

```yaml frontmatter invalid="on.watch.every: \"often\" is not a duration — use 1m, 5m, 1h"
name: vague-watch
description: Says how often in words.
on:
  watch:
    script: scripts/watch.js
    every: often
```

```yaml frontmatter invalid="on.watch.every: 30s is shorter than a minute — use 1m or more"
name: inbox-watch
description: React to new messages.
on:
  watch:
    script: scripts/check.js
    every: 30s
```

```yaml frontmatter invalid="on.watch.every: 48h is longer than 24h — use on.schedule for anything rarer"
name: slow-watch
description: Watches every two days.
on:
  watch:
    script: scripts/check.js
    every: 48h
```

## `hooks`

```yaml frontmatter invalid="hooks: must be a mapping with pre and post lists"
name: listed-hooks
description: Gives the hooks as a list.
on:
  manual: true
hooks:
  - script: scripts/check.sh
```

```yaml frontmatter invalid="hooks.after: unknown key — hooks has pre, post"
name: after-hooks
description: Uses a hook list that does not exist.
on:
  manual: true
hooks:
  after:
    - script: scripts/save.sh
```

```yaml frontmatter invalid="hooks.pre: must be a list of { script: <path> } or { prompt: <text> }"
name: single-hook
description: Gives one hook instead of a list.
on:
  manual: true
hooks:
  pre: scripts/check.sh
```

```yaml frontmatter invalid="hooks.pre[0]: must be { script: <path> } or { prompt: <text> }"
name: triage
description: Triage new issues.
on:
  manual: true
hooks:
  pre:
    - script: scripts/new_issues.sh
      prompt: Ignore duplicates.
```

```yaml frontmatter invalid="hooks.pre[0].prompt: must be text — an empty prompt asks nothing"
name: silent-prompt
description: Has a prompt hook with nothing to ask.
on:
  manual: true
hooks:
  pre:
    - prompt: ""
```

```yaml frontmatter invalid="hooks.pre[0].script: is empty — give a path relative to the automation directory"
name: empty-script
description: Has a script hook with no path.
on:
  manual: true
hooks:
  pre:
    - script: ""
```

```yaml frontmatter invalid="hooks.pre[0].script: \"/usr/local/bin/check.sh\" is absolute — use a path relative to the automation directory"
name: absolute-hook
description: Calls a script outside its folder.
on:
  manual: true
hooks:
  pre:
    - script: /usr/local/bin/check.sh
```

```yaml frontmatter invalid="hooks.post[0].script: \"../shared/post.sh\" leaves the automation directory — scripts must live inside it"
name: team-digest
description: Post the team digest.
on:
  manual: true
hooks:
  post:
    - script: ../shared/post.sh
```

## `flow`

```yaml frontmatter invalid="flow: \"tight\" is not loose or strict"
name: tight-flow
description: Asks for a flow that does not exist.
flow: tight
on:
  manual: true
```

With `flow: strict`, every problem between the body and its chart is reported in one line; the rules and their messages are in [Strict flow](flowcharts.md#strict-flow).

~~~markdown automation dir=strict-no-chart invalid="flow: the body has no flowchart — add a ```mermaid block that starts with flowchart TD and has a node per step, or remove flow: strict"
---
name: strict-no-chart
description: Asks for a strict flow without a chart.
flow: strict
on:
  manual: true
---

Read the issue, then write a verdict.
~~~
