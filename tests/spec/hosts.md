---
title: Hosts
description: Agents that run Agent Automations, and what a host may add.
order: 4
---

An `AUTOMATION.md` is only a file until a host runs it. A host discovers automations, keeps their schedules and watches, asks for consent, runs them and records what happened. Everything on the [Specification](specification.md) page applies to every host. This page covers what a host may add on top.

## What a host adds

The format fixes what an automation means. A host chooses how people work with it:

- Its interface: how you list automations, check one, arm it, run it by hand and read its records. It can be a command line, a page in the agent, or both.
- Its defaults and limits: the default and maximum `timeout`, the hook timeout, the grace period for a late tick, and whether consent expires.
- More folders: a host **may** also scan folders of its own, beside `.agents/automations/`.
- Optional triggers: a host **may** leave `on.watch` out. It then refuses an automation that uses it, by name, and never runs the rest.
- Reporting: progress on the flowchart, notes of what a run published, and one message for the reader. Draft 0.2.0 does not define these. A host that has them can tell the agent how to use them at run time, so a body can ask for them in plain words.
- Plugins: ways to observe runs, add actions or draw flowcharts. Each host defines its own.

Before you rely on one of these in an automation you share, check that every host your team uses has it.

## Known hosts

| Host | Agent | Pages |
|---|---|---|
| fm, the reference host: a command line and terminal UI | GitHub Copilot CLI, Claude Code, Codex CLI, Gemini CLI, OpenCode and more | [fm](https://github.com/lucas-stanford/fm) |
| pi-automations, an extension for pi that runs on fm | [pi](https://pi.dev) | [pi-automations example](pi-automations.md) |

fm implements every page of this site, including the optional [flowcharts](flowcharts.md) and [`nodes`](flowcharts.md#nodes-field), and its tests check every example here against it. `fm validate <AUTOMATION.md>` checks a file the same way.

If you are adding the format to an agent, start with [Adding automations support](adding-support.md).
