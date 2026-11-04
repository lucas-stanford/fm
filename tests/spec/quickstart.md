---
title: Quickstart
description: Create your first automation, run it, and read its record.
group: For automation authors
order: 1
---

You will write an automation that greets you with the date, have your host check it, arm it, run it by hand, read its record, and put it on a schedule. It takes about five minutes.

## Prerequisites

- An agent with a host that implements Agent Automations Draft 0.1.0. The [Hosts](hosts.md) page lists the ones known.
- A model you are logged in to. Scheduled runs do not see API keys from your shell profile.

> [!NOTE]
> Each host has its own commands for checking, arming and running. This page says what to do; your host's documentation says how. With pi, follow [Quickstart with pi](quickstart-pi.md): it has the exact commands.

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

Ask your host to check the file. It should list `hello-world` as **off**: the file is valid, but nothing will run it yet. A new automation stays off until you arm it, even in your home directory.

If you made a mistake, the automation is **invalid**, and one line says what is wrong and how to fix it, for example `name: "hello_world" is not a valid name — use lowercase letters, digits and single hyphens`.

## Arm it

Arming is your consent to run exactly this file on this machine. Before it asks, the host shows a consent preview of everything the automation would run: its triggers, model, timeout, hooks and the exact command each script will run. Read the preview, then accept it.

Consent is pinned to the content of `AUTOMATION.md` and its scripts. Change any of them and the automation is off until you arm it again.

## Run it

Ask your host to run `hello-world` now. The run starts a fresh session, sends it the body, and waits for the answer.

## Read the record

Every run leaves a record: when it ran, how it ended, how long it took and what the agent said. Open it in your host and check that the run ended `ok` with the greeting you expected.

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

Saving the change turned the automation off, because the file is no longer the one you armed. The host says so and names the file that changed. Read the new preview, which now shows the schedule, and arm the automation again.

Schedules need a process that wakes up every minute. For them to fire while the agent is closed, find out how your host does that and start it.

## How it works

1. The host found `hello-world/AUTOMATION.md` under `~/.agents/automations/`, checked its frontmatter, and listed it as **off**.
2. You armed it after reading the preview. The host recorded what you agreed to, and returns the automation to off whenever the file or one of its scripts changes.
3. Running it by hand (and, later, the schedule) started a run: pre hooks (none yet), then the body in a new session, then post hooks (none yet).
4. The run ended `ok` and the host wrote its record.

A file in a repository's `.agents/automations/` works the same way, and everyone who clones the repository sees its preview and arms it on their own machine.

## Next steps

- Add a `pre` hook that gathers facts with a script, so the agent starts from data instead of fetching it: see the [`hooks` field](specification.md#hooks-field).
- Start runs only when there is something to do, with [a watch script](watch-scripts.md).
- Ask each run for receipts: a record of what it published and one message for you, with links. See [Ask for receipts](best-practices.md#ask-for-receipts).
- Read the [best practices](best-practices.md) before you share an automation with your team.
