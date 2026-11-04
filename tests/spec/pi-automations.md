---
title: pi-automations
nav: Overview
description: An example host, built into the pi coding agent. How to install and use it, and what it adds to the specification.
group: "Example: pi-automations"
order: 1
---

[pi-automations](https://github.com/lucas-stanford/pi-automations) is an example of adding the format to an existing agent: an extension for the [pi coding agent](https://pi.dev). It runs each automation in the background, so a run keeps going while pi is closed, and it records every run with its steps, what it said and what it published.

The pages in this example are about pi-automations only. Everything else on this site applies to every host.

## What it supports

| Feature | pi-automations |
|---|---|
| Discovery in `~/.agents/automations` and `<repo>/.agents/automations` | Yes, plus `~/.pi/agent/automations` and `.pi/automations` |
| `on.manual` | Yes: `/automations run`, `pa run` |
| `on.schedule` with `timezone` and `catch-up` | Yes, with pi open or closed |
| `on.watch` | Yes (introduced here) |
| `script` and `prompt` hooks | Yes |
| Automations off until armed, in both scopes | Yes, pinned to a hash; repository consent expires after 30 days |
| Run records | Yes: every outcome, with hook results, session and notes |
| Progress on the flowchart | Yes, with `pa step` and `pa next` |
| `flow: strict` | Yes: every problem listed by `pa check`, and a strict run's `pa next` follows the chart's arrows |
| Notes with links | Yes, with `pa note` |
| One message per run, for the reader | Yes, with `pa message`: shown on the `/automations` page and in the Automation Manager's History |

## Install

```sh
pi install git:github.com/lucas-stanford/pi-automations
```

It needs Node.js 22.19 or later and pi 0.84.4 or later. Then open pi and run `/automations` (or press `ctrl+alt+a`). To keep schedules and watches firing while pi is closed, start the watcher, which fires schedules and runs watch checks, once: `/automations watcher start`, or `w` on the `/automations` page. [Quickstart with pi](quickstart-pi.md) walks through a first automation.

The package also ships two agent skills: `agent-automations`, for writing an automation on any host, and `pi-automations`, for checking, arming, running and reporting with `pa`.

## Inside pi: `/automations`

The `/automations` page puts what needs you first: its Needs you section holds failed runs and automations waiting for consent. Running now, Coming up, Watching, and Off and disabled follow. Every action is one key, listed at the bottom of the page.

## From a shell: `pa`

`pa` (also installed as `pi-automations`) has the same verbs as `/automations`:

| Command | Does |
|---|---|
| `pa` · `pa list` | List automations and their state |
| `pa check [name]` | Re-check everything, or show every detail of one automation |
| `pa run <name>` | Run one now and wait. Exits `0` ok, `10` skipped, `20` failed, `30` aborted |
| `pa runs <name> [n]` | Recent runs, with what each said and published |
| `pa running` | Runs in flight |
| `pa arm <name>` · `pa disarm <name>` | Consent to an automation, or withdraw it. `pa arm` prints the preview; add `--yes` to accept it |
| `pa enable <name>` · `pa disable <name>` | Flip `enabled:` in a user automation. Consent it had is kept |
| `pa watch [name]` | Inspect watches, or run one check now without starting anything |
| `pa watch pause <name>` · `pa watch resume <name>` | Pause and resume one automation's watch checks |
| `pa edit <name>` | Open it in an editor |
| `pa report [--open]` | Write a self-contained HTML report |
| `pa watcher start\|stop\|status` | The watcher, which fires schedules and runs watch checks |
| `pa tick` · `pa serve` | Tick once, or tick every minute in the foreground (for containers) |

Inside a run, `pa` also reports progress (`pa step`, `pa next`, `pa say`), notes (`pa note`) and the run's one message for you (`pa message`): see [Reporting progress and notes](reporting.md).

## Testing an automation

1. Start with `on: { manual: true }`, check it with `pa check <name>`, and arm it after reading its preview: `pa arm <name>`, then `pa arm <name> --yes`, or `a` in `/automations`.
2. Run it by hand with `pa run <name>` until the output is right, and read each record with `pa runs <name>`. Every edit turns it off until you arm it again, so you see each change in the preview.
3. With `flow: strict`, run `pa check <name>` while you write: it lists every mismatch between the chart and the sections, with its line.
4. For a watch, run `pa watch <name>` to see what one watch check would do without starting anything.
5. Then add the schedule or the watch, keep `manual: true` so you can still run it on demand, and arm it once more.

### Trying a watch

`pa watch <name>` runs one watch check now and says what a tick would do with it. It starts nothing and records nothing.

```sh
pa watch pr-review
```

```text
pr-review: ran scripts/review_requests.mjs in 1s — nothing was started or recorded
  result        exit 0
  a tick would  start a run: it found something new
  output
    https://github.com/OWNER/pi-automations/pull/7
```

It exits `0` when a tick would start a run, `10` when it would not, and `20` when the script could not be checked. `pa watch` without a name lists every watch with its last check and the next one. `pa watch pause <name>` pauses the checks without touching the file; `pa watch resume <name>` resumes them.

## The watcher

Schedules and watches need something that wakes up every minute. The watcher is one small job pi-automations registers with the operating system (Task Scheduler on Windows, launchd on macOS, a systemd user timer or cron on Linux). It runs one tick a minute whether pi is open or not. `pa watcher start` starts it, `pa watcher stop` stops it, and `pa watcher status` says whether it is running. `pa serve` runs the same tick in the foreground, for containers. Nothing fires without one of them.

Scheduled runs do not read your shell profile, so log in with `pi /login` rather than relying on API-key environment variables.

## State and configuration

Consent, run records, locks, logs and reports live in `~/.pi/agent/pi-automations/`, never inside a repository. `~/.pi/agent/pi-automations/config.json` holds the host policy. Every key is optional:

```jsonc
{
  "grace": "10m",            // how late a tick may be and still count as on time
  "catchUpWindow": "7d",     // how far back catch-up: once looks
  "defaultTimeout": "20m",
  "maxTimeout": "4h",
  "hookTimeout": "60s",      // per hook; watch scripts get at most 2m
  "consentExpiry": "30d",     // repository automations only; user consent does not expire
  "notifyOnFailed": true,    // a desktop notification when a run fails
  "plugins": ["manager"]
}
```

## What it adds to the specification

The [Specification](specification.md) leaves some choices to the host. pi-automations makes these:

- More folders: it also reads `~/.pi/agent/automations/` (user scope) and `.pi/automations/` (repository scope).
- Timeouts: a run without `timeout` gets `20m`. A `timeout` above `4h` makes the automation invalid. Each hook gets `60s`, and a watch script at most two minutes.
- Grace period: a tick up to 10 minutes late still counts as on time; past that, `catch-up` applies, looking back at most 7 days.
- Watch checks: a watch check counts as due up to 30 seconds early, so tick jitter does not add a minute.
- Consent: consent to a repository automation expires after 30 days by default. Consent to a user automation lasts until its files change or you disarm it.
- `on.watch`: it was introduced here, as the first trigger beyond `schedule` and `manual`.
- More environment: hooks, the body and everything they start also get `AUTOMATION_RUN`, `AUTOMATION_STATE_DIR` and, on a tracked run, `AUTOMATION_NODE`, for [progress reporting](reporting.md).
- Strict runs: with `flow: strict`, a run's `pa next` must follow an arrow of the flowchart, or pi-automations refuses it with `NOT_AN_EDGE`. The host makes the moves between hooks and the body itself, and does not check them.

## Plugins and the Automation Manager

Plugins observe runs, add actions and draw flowcharts. They are loaded only from your own `config.json`, never from a repository, because a plugin runs with your permissions in every session. `"plugins": ["manager"]` loads the bundled Automation Manager: press `e` on an automation to edit its triggers, hooks, body and flowchart in a browser, follow its runs on their charts, and read every run's message in History.

## Drawing charts

Progress and notes need no extra tool. Without a flowchart renderer, a run page shows the chart as a list of steps. If an `fm` that answers `fm render --help` is installed, pi-automations uses it to draw the chart in the terminal and in the HTML report.
