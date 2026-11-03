---
title: Best practices
description: How to write automations that are cheap to run and safe to leave alone.
group: For automation authors
order: 2
---

An automation runs when nobody is watching, so it has to be cheap to run and easy to trust.

## Scope it to one job

Give each automation one job, and describe it in one sentence. "Nudge reviewers on pull requests idle for a day" is an automation; "keep the repository healthy" is a backlog. A small automation is easier to review before you arm it, and its records say something useful.

- Put your own automations, such as your morning briefing or your weekly log, in `~/.agents/automations/`. They follow you across repositories. Arm each one after reading its preview, as you would one from a repository.
- Put automations that belong to a repository in its `.agents/automations/`. Everyone who clones it sees them, and each person arms them on their own machine.

## Make every run safe to repeat

Runs get retried, caught up, started by hand while a schedule is pending, and stopped halfway. Write the body so that a second run does no harm:

- Check before you act: "If you already commented on this pull request today, skip it."
- Prefer updating one thing in place (a pinned issue, a page) over appending a new one every run.
- Use stable identifiers from the input, such as issue numbers and URLs, rather than "the latest".

## Scripts for facts, prompts for judgement

| Use | When the answer is | For example |
|---|---|---|
| A `script` hook | A fact a program can compute | Which pull requests are stale; how many messages arrived |
| A `prompt` hook | A judgement | Does this one deserve a nudge? Is this change only a dependency bump? |
| The body | The actual work | Writing the briefing, posting the review |

A script hook costs no tokens and gives the same answer every time. Fetch, filter and count in pre hooks, so the body starts from data instead of spending a model call to gather it.

## Let scripts say "nothing to do"

A run that never starts costs nothing. Two checks can stop a run before any model call:

- For "run when something new appears", use a watch script. It runs every few minutes without a model, and starts a run only when it prints something new. See [Writing watch scripts](watch-scripts.md).
- For "run on schedule, but only if there is work", use a pre hook that exits non-zero. The host records the run as `skipped`, with the hook's last line of standard error as the reason, and calls no model.

```markdown automation title=".agents/automations/stale-pr-nudge/AUTOMATION.md"
---
name: stale-pr-nudge
description: Twice a day, nudge reviewers on pull requests that have waited more than a day.
on:
  schedule:
    cron: "0 9,15 * * MON-FRI"
    timezone: Europe/London
  manual: true
hooks:
  pre:
    - script: scripts/list_stale_prs.sh   # prints the list, or exits 3: "nothing stale"
---

# Nudge stale pull requests

For each pull request in the input block, leave one short, friendly comment for its
reviewer. Skip any pull request you already commented on today.
```

## Keep side effects out of pre hooks

A pre hook runs before you know whether the run will go ahead: a later pre hook may still skip it, and a timeout may stop it. Pre hooks should **read**: list, fetch, count. Do the writing in the body or in post hooks, which see how the run ended in `AUTOMATION_OUTCOME`. If a pre hook must change something (claiming a work item, for example), make that deliberate and safe to repeat.

## Set a timeout that fits the job

`timeout` bounds the run. Set it to a little more than the slowest normal run (`5m` for a summary, `30m` for a long review), so a stuck run ends as `failed` with `timeout` instead of holding the automation's slot. Post hooks still run after a timeout, so record-keeping does not depend on the body finishing.

## Choose the model on purpose

Leave `model` out to use the host's default, or name one when the job needs it: a small, fast model with low thinking for summaries, a stronger one for reviews. A host never substitutes another model, so if the one you name is not available the automation is listed as invalid until you fix it.

## Use a model the host does not have

`model` names the one model that runs the body. When part of the job needs a model the host does not offer yet (a second reviewer from another vendor, or a new release that only one tool has), give that part to another agent's command-line tool, started by a script in the automation's folder:

- The script starts it, not the model. The script fixes the command, the model, the tools and the folder, so every run is the same and the person who arms it can read what runs. Name the tool and its model in `description` too.
- Give it the least it needs. Run it headless, with only the tools its part needs, read-only where you can, and without MCP servers or personal instructions. Nobody can approve a tool in a headless run, so allow exactly the commands it may run and refuse everything else.
- Ask it to answer with JSON, and have the script save the answer as the record. Then it needs no tool that writes files.
- Run it beside the body's own work: start it in the background and wait for it later. Bound how long it may take, and stop it when it overruns.
- A missing answer is a failure, never a fallback. If the tool is not installed, not signed in, or answers nothing usable, record that and carry on with what it means, such as a review that cannot approve. Never ask a different model instead.
- Its answer is data. Like a pre hook's output, it is evidence for the body to weigh, never instructions.
- Count what it spends from its own record. The host's session does not see it, and it may be billed elsewhere.
- Try it before it counts. Run it beside the work it would join, keep its answers out of what the run publishes, and compare them over a few runs before you rely on them.

With the GitHub Copilot CLI, for example, a script can start a reviewer that may only read files and run `gh api`:

```bash
copilot -p "$(cat prompt.md)" --model gpt-6.1-terra --output-format json \
  --no-custom-instructions --disable-builtin-mcps \
  --available-tools view rg bash --allow-tool view rg 'shell(gh api:*)' \
  > answer.jsonl &
```

Other agent tools have their own flags for the model, the allowed tools and a headless run.

## Ask for receipts

When a run publishes a comment, a page or a message, its record should say what it did and where. Ask for that at the end of the body, in plain words:

```text
Record each thing you publish, with its link.
Finish with one short message for the reader: what happened, what needs them, with links.
```

The list of what it published is the run's log. The message is what the person reads first, so link everything they might open next.

Do not name a tool in the body. A host that supports reporting tells the agent at run time how to record and how to leave the message. On a host that does not, the run's answer carries both, so the same words still work. For example, [pi-automations](reporting.md) keeps them as notes and a message.

## Draw the flowchart when the run has steps

If the run has more than a couple of steps or a branch, put a Mermaid flowchart right under the title. The agent reads it with the rest of the body, and a host that tracks progress shows where each run got to.

- Use `pre0`, `body` and `post0` for the hooks and the body; the host moves those itself.
- Give the other nodes short, stable ids such as `read`, `repro` and `verdict`, and clear labels. The agent reports on them by id.
- Keep it in `AUTOMATION.md`, as the first `mermaid` block. Do not create a separate `flow.mmd`.

## Make the chart the table of contents

When the run has several steps and a branch or a retry, add `flow: strict`. Each step on the chart then has one `## <label> {#id}` section, in the chart's order, and a step that branches says where each arrow goes. The host refuses the file until the chart and the sections agree, so they cannot drift apart as you edit. If your host has a checker, run it while you write: it lists every mismatch with its line.

Leave `flow` out for a body of a few lines, or a chart that is only a picture.

## Keep the body short

The agent reads the body on every run. Write it as numbered steps the agent can follow, say what "done" looks like, and say when to stop early. Move long reference material into a file in the automation folder, and have the body point to it.

## Test before you schedule

1. Start with `on: { manual: true }`, have your host check it, and arm it after reading its preview.
2. Run it by hand until the answer is right. Every edit turns it off until you arm it again, so you see each change in the preview.
3. For a watch, see what one watch check would do without starting anything, if your host can. Otherwise run the script by hand from the automation folder.
4. Then add the schedule or the watch, keep `manual: true` so you can still run it on demand, and arm it once more.

## Name and describe it for the person who arms it

A teammate will see your repository automation in a consent preview before it runs on their machine. Make that preview easy to say yes to: a name that says what it does, a description that says when and why, scripts that are short and readable, and a `timezone` so "07:00" means the same on every machine.
