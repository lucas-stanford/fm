---
title: How pi-automations implements it
nav: How it implements the spec
description: The choices pi-automations makes where the specification leaves them to the host, as an example for people building their own.
group: "Example: pi-automations"
order: 4
---

[Adding automations support](adding-support.md) says what every host has to do. This page says how one example host, pi-automations, does it, section by section, as choices to reuse or depart from.

## Discover

- Besides `~/.agents/automations/`, it reads `~/.pi/agent/automations/` (user scope). Besides `.agents/automations/`, it reads `.pi/automations/` (repository scope).
- Each automation's identity is `<scope>:<absolute folder>:<name>`.
- It remembers a repository when one of its automations is armed, so the watcher knows where to look when no repository is open.

## Validate

pi-automations checks the cron against the Draft 0.1.0 cron grammar first and uses [croner](https://github.com/Hexagon/croner) only for date arithmetic. A `timeout` above its maximum (`4h` by default) makes the automation invalid.

## Consent

pi-automations hashes `AUTOMATION.md`, the watch script and the hook scripts with their relative paths, and keeps a hash per file so it can say which one changed. Consent to a repository automation expires after 30 days (`consentExpiry`). Consent to a user automation does not expire: it lasts until the files change or the person disarms it.

## Keep the clock

- pi-automations registers the watcher, which fires schedules and runs watch checks: one small job with the operating system (Task Scheduler on Windows, launchd on macOS, a systemd user timer or cron on Linux) that runs a tick each minute. `pa serve` ticks in the foreground, for containers.
- The grace period is 10 minutes, and `catch-up: once` looks back at most 7 days.
- When many automations are due at the same minute, it delays each by a fixed offset derived from its identity, at most two minutes and at most half its shortest period.
- It takes a tick lock and replaces it when its owner is dead or it is older than five minutes, so a crash cannot stop the clock.
- A watch check counts as due up to 30 seconds early, so tick jitter does not add a minute.

## Run

- It runs the steps in the order on [Adding automations support](adding-support.md#run).
- Each hook gets 60 seconds and 1 MiB of output by default (`hookTimeout`, `hookMaxOutputBytes`); a watch script at most two minutes.
- To stop a run, it writes a stop request to a file that the run polls, because on Windows a signal kills the process outright and leaves its lock behind.
- Hooks, the body and everything they start also get `AUTOMATION_RUN`, `AUTOMATION_STATE_DIR` and `AUTOMATION_NODE`, for [progress reporting](reporting.md).

## Record

pi-automations appends one JSON line per run to a file per automation in its state directory, `~/.pi/agent/pi-automations/`, and trims old lines.

## Building on pi-automations

pi-automations is MIT-licensed, and its rules can be reused without pi. The portable core (`dist/core/index.js`) owns discovery, consent, schedules, locks, hooks, records and progress, and loads nothing at import. A host composes its own agent runtime, script executor, worker launcher and notifier around it; [docs/PROVIDERS.md](https://github.com/lucas-stanford/pi-automations/blob/main/docs/PROVIDERS.md) describes each contract, and the repository's tests run a complete host built from the core alone.

pi-automations also has a plugin API for people who use it, not for automations. Plugins are named only in the user's own config file, never in a repository, because a plugin runs with the user's permissions everywhere. A plugin can observe runs (`run-start`, `step`, `note`, `run-end`), add actions to the `/automations` page and the `pa` command, draw charts, and use the host's own parser and records. It cannot change a run: observers are called after the fact with a time limit, and a plugin that fails is recorded on the run and otherwise ignored. See [docs/PLUGINS.md](https://github.com/lucas-stanford/pi-automations/blob/main/docs/PLUGINS.md).
