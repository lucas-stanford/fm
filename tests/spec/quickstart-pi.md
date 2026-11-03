---
title: Quickstart with pi
nav: Quickstart
description: Create your first automation in pi, arm it, run it with pa, and read its record.
group: "Example: pi-automations"
order: 2
---

You will write an automation that greets you with the date, arm it, run it by hand, read its record, and put it on a schedule. It takes about five minutes.

## Prerequisites

- [pi](https://pi.dev) with [pi-automations](pi-automations.md#install) installed.
- A model you are logged in to with `pi /login`. Scheduled runs do not see API keys from your shell profile.

> [!NOTE]
> This tutorial uses pi-automations, but the file you write is plain Agent Automations Draft 0.2.0. Any host that implements the format runs it the same way. The [Quickstart](quickstart.md) has the same steps for any host.

## Create the automation

An automation is a folder containing an `AUTOMATION.md` file, and the folder name is the automation's name. Create `~/.agents/automations/hello-world/AUTOMATION.md`:

```markdown automation title="~/.agents/automations/hello-world/AUTOMATION.md"
---
name: hello-world
description: Greet me with today's date and one thing worth knowing about it.
on:
  manual: true
---

# Hello

Read the `now` attribute of the `<run>` element in the input block below.
Reply with one friendly sentence that says hello and gives today's date and weekday,
then one short sentence about something that happened on this date in history.
```

The frontmatter between the `---` lines says when the automation runs: `on.manual: true` means "when I ask". The Markdown body is the prompt for each run.

## Check it

Check the file. In pi, run `/automations check hello-world`; from a shell, `pa check hello-world`:

```sh
pa check hello-world
```

```text
hello-world  off
  reason        not armed — /automations arm hello-world
  scope         user
  directory     ~/.agents/automations/hello-world
  ...
```

`off` means the file is valid but nothing will run it yet. A new automation stays off until you arm it, even in your home directory. If you made a mistake, the state is `invalid` and the next line says what is wrong and how to fix it, for example `name: "hello_world" is not a valid name — use lowercase letters, digits and single hyphens`.

## Arm it

Arming is your consent to run exactly this file on this machine. `pa arm` prints a consent preview of everything the automation would run, and changes nothing:

```sh
pa arm hello-world
```

```text
Arm hello-world  from ~/.agents/automations
description   Greet me with today's date and one thing worth knowing about it.
manual        allowed
model         host default
timeout       20m (host default)

This arms a schedule on this machine with your credentials. Scripts run with your permissions.

not armed — re-run with --yes to accept, or use /automations arm inside pi
```

Read it, then accept it:

```sh
pa arm hello-world --yes
```

In pi, `/automations arm hello-world` shows the same preview and asks; press `y`. On the `/automations` page, select it and press `a`.

Consent is pinned to the exact bytes of `AUTOMATION.md` and its scripts. Change any of them and the automation is `off` until you arm it again.

## Run it

```sh
pa run hello-world
```

The run starts a fresh pi session in the automation's folder, sends it the body, and waits for the answer. `pa run` exits `0` when the run ended `ok`. In pi, `/automations run hello-world` does the same, or select it on the `/automations` page and press `r`.

## Read the record

Every run leaves a record: when it ran, how it ended, how long it took and what the agent said.

```sh
pa runs hello-world
```

On the `/automations` page, press `enter` on the automation to open its automation page, with its last run and the session it ran in.

## Put it on a schedule

Add a schedule beside the manual trigger. This one runs at 08:30 on weekdays, in New York time:

```markdown automation title="~/.agents/automations/hello-world/AUTOMATION.md"
---
name: hello-world
description: Greet me with today's date and one thing worth knowing about it.
on:
  schedule:
    cron: "30 8 * * MON-FRI"
    timezone: America/New_York
  manual: true
---

# Hello

Read the `now` attribute of the `<run>` element in the input block below.
Reply with one friendly sentence that says hello and gives today's date and weekday,
then one short sentence about something that happened on this date in history.
```

Saving the change turned the automation off, because the file is no longer the one you armed:

```sh
pa check hello-world
```

```text
hello-world  off
  reason        changed since armed: AUTOMATION.md — /automations arm hello-world
  ...
```

The new preview shows the schedule. Read it and arm the automation again:

```sh
pa arm hello-world
pa arm hello-world --yes
```

Schedules need something that wakes up every minute, even when pi is closed. Start the watcher, which fires schedules and runs watch checks, once:

```sh
pa watcher start
```

`pa check hello-world` now says `armed` and lists the next three times it will run.

## How it works

1. pi-automations found `hello-world/AUTOMATION.md` under `~/.agents/automations/`, checked its frontmatter, and listed it as **off**.
2. You armed it after reading the preview. pi-automations recorded a hash of the file, and returns the automation to off whenever the file or one of its scripts changes.
3. `pa run` (and, later, the schedule) started a run: pre hooks (none yet), then the body in a new session, then post hooks (none yet).
4. The run ended `ok`, and pi-automations wrote its record to `~/.pi/agent/pi-automations/`.

A file in a repository's `.agents/automations/` works the same way, and everyone who clones the repository sees its preview and arms it on their own machine.

## Next steps

- Add a `pre` hook that gathers facts with a script, so the agent starts from data instead of fetching it: see the [`hooks` field](specification.md#hooks-field).
- Start runs only when there is something to do, with [a watch script](watch-scripts.md).
- Record what each run published with [`pa note`](reporting.md), and end it with one [`pa message`](reporting.md#the-message) for you, with links.
- Read the [best practices](best-practices.md) before you share an automation with your team.
