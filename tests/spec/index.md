---
title: Agent Automations Overview
nav: Overview
description: A standardized way to tell AI agents when to act.
order: 1
---

## What are Agent Automations?

Agent Automations are a small, open file format for running an AI agent without anyone typing a prompt: on a schedule, when a script finds new work, or on demand.

An automation is a folder containing an `AUTOMATION.md` file. Its YAML frontmatter says **when** the agent runs and **what runs before and after it**: scripts or short prompts called hooks. Its Markdown body is the prompt for every run. The folder can also hold the scripts the hooks call and any other file the body refers to.

```tree
daily-briefing/
├── AUTOMATION.md        # Required: frontmatter and body
├── scripts/             # Optional: hook and watch scripts, by relative path
│   └── overnight_counts.sh
└── ...                  # Any other files the body refers to
```

This one runs at 07:00 on weekdays and can also be run by hand. It starts from numbers a script gathered without a model call:

```markdown automation title="~/.agents/automations/daily-briefing/AUTOMATION.md"
---
name: daily-briefing
description: Every weekday morning, summarise what changed overnight and what needs attention today.
timeout: 10m
on:
  schedule:
    cron: "0 7 * * MON-FRI"
    timezone: America/New_York
  manual: true
hooks:
  pre:
    - script: scripts/overnight_counts.sh
---

# Daily briefing

Start from the counts in the input block below. If every count is zero, say so in one
line and stop. Otherwise list what needs attention, most urgent first, and finish with
the three things worth doing today.
```

## Why Agent Automations?

Most agent work still starts with a person typing a prompt. Agent products that add a scheduler each invent their own file and rules. Agent Automations define one file that any host can run:

- The schedule travels with the prompt, in a file you can copy to any other host that implements the format.
- Watch scripts and pre hooks run without a model and decide whether there is anything to do, so an hour with no new work costs no tokens.
- Every run leaves a record of when it ran, how it ended, what each hook did and what it published.
- A new automation stays off until you have seen exactly what it would run and armed it, wherever it came from.

## How do Agent Automations work?

A host, the program that implements the format, takes every automation through the same stages. Only the frontmatter is read until a run starts.

1. Discovery: the host scans `.agents/automations/*/AUTOMATION.md` in your home directory and in the repository you are working in, and checks every file against the [specification](specification.md).
2. Consent: every automation, in your home directory or in a repository, stays **off** until you review a preview and arm it. Editing the file or any of its scripts returns it to off.
3. Trigger: an armed automation starts on its cron schedule, when its watch script reports new work, or when a person runs it.
4. Pre hooks: scripts and prompts run in order. The host hands their output to the body as data, and any of them can skip the run.
5. Body: the agent gets the body as its prompt in a fresh session, followed by the pre-hook output.
6. Post hooks: they always run, see what the body answered and how the run ended, and can record or notify.
7. Record: the run ends as `ok`, `skipped`, `failed`, `missed` or `aborted`, and the record says why.

## Where can I use Agent Automations?

An `AUTOMATION.md` does nothing on its own: a host has to discover it, keep the schedule and run it. [fm](https://github.com/lucas-stanford/fm), the reference host, runs automations through agent CLIs such as GitHub Copilot CLI, Claude Code and Codex CLI. The [Hosts](hosts.md) page lists the known hosts and what a host may add, and [Adding automations support](adding-support.md) explains how to implement the format in another agent.

## Format version

This site describes **Draft 0.2.0**, the initial specification. The frontmatter has a closed list of nine fields. A conforming host refuses anything it does not implement, names it, and says in one line what would fix it, so it never runs something different from what the author wrote. The optional [`flow`](flowcharts.md#flow-field) field controls [flowchart](flowcharts.md) tracking, and the optional [`nodes`](flowcharts.md#nodes-field) field hands steps to their own agent.

## Get started

<!-- cards -->
- [Quickstart](quickstart.md): Create your first automation, run it and read its record in five minutes.
- [Specification](specification.md): The complete AUTOMATION.md format, with every field, trigger, hook and rule.
- [Hosts](hosts.md): Where automations run today, and what a host may add.
