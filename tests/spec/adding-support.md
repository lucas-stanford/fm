---
title: Adding automations support
nav: Adding automations support
description: How to implement Agent Automations in an agent or tool, from discovery to the run record.
group: For host implementors
order: 1
---

This page is for people adding Agent Automations to an agent, an IDE or a CLI. Its sections follow a host's work in order and mark which parts are required. The [Specification](specification.md) is the authority on the format. For one example host's choices, see [How pi-automations implements it](pi-implementation.md).

## What a host does

1. Discover every `AUTOMATION.md` in the user's and the repository's `automations/` folders.
2. Validate each one completely, before anything runs, and list the ones it cannot honour with one line saying why.
3. Ask for consent before arming anything, in either scope.
4. Keep the clock: fire schedules on time, catch up or record what was missed, and run watch scripts if it supports `on.watch`.
5. Run: one run per automation at a time, pre hooks, the body in a fresh session, post hooks, all within the timeout.
6. Record every run, including the ones that were skipped before they started.

Conformance is small on purpose: scan two folders, parse eight fields, keep a cron, run two hook lists, and keep every automation off until it is armed.

## Discover

Scan these folders one level deep. Each subdirectory that contains an `AUTOMATION.md` is an automation; one that does not is skipped silently.

| Scope | Folders |
|---|---|
| User | `~/.agents/automations/` |
| Repository | `.agents/automations/` in the working directory and every parent up to the repository root, nearest first. The repository root is the nearest directory with a `.git` entry. |

A host **may** scan other folders of its own. Give each automation a stable identity from its scope, its folder and its name, such as `<scope>:<absolute folder>:<name>`: two folders can hold automations with the same name.

Scheduled work happens when nobody has a repository open, so a host that fires repository automations from a background process needs to remember which repositories to look in, for example each repository in which an automation is armed.

## Validate

Parse the frontmatter as **YAML 1.2**, so `on` stays a key (YAML 1.1 reads it as `true`), with duplicate keys refused and a key with an empty value treated as absent. Then check in this order and report the first problem:

1. The `---` lines and the YAML, then a non-empty body.
2. Unknown top-level fields, all of them in one line.
3. `name`: present, a string, at most 64 characters, `^[a-z0-9]+(-[a-z0-9]+)*$`, equal to the folder name.
4. `description`: present, a string, at most 1024 characters.
5. `enabled`: a boolean if present.
6. `model`: `<provider>/<id>` with an optional `:<thinking>`.
7. `timeout`: a positive integer followed by `s`, `m` or `h`.
8. `on`: a mapping with at least one trigger, every trigger one this host implements.
9. `hooks`: only `pre` and `post`, each a list of `{ script }` or `{ prompt }` items.
10. `flow`: `loose` or `strict`. With `strict`, check the body against its chart by the [Strict flow](flowcharts.md#strict-flow) rules, and report every problem in one line.

Every message has the same shape, `<field path>: <what is wrong> — <what fixes it>`, so the [validation messages](validation.md) are also a test suite.

> [!IMPORTANT]
> Refuse what you do not implement, by name. A host without `on.watch` refuses the automation with `on.watch: this host implements schedule, manual`. It never ignores the trigger and runs the rest: an automation that runs differently from what its author wrote is worse than one that does not run.

Validate the cron against the Draft 0.1.0 cron grammar yourself before handing it to a cron library. Most libraries accept more (`@daily`, `L`, `#`, six fields), and a host that accepts them makes files that fail on other hosts. Check the grammar first, and use the library only for date arithmetic.

Some checks need the machine as well as the file. Do them at discovery too, so a run never fails at 07:00 for a reason that was knowable at 18:00:

- Scripts stay inside the folder. Refuse absolute paths and `..`, and resolve symbolic links: `on.watch.script: "scripts/watch.js" resolves outside the automation directory`.
- Scripts can run here. Pick the interpreter from the extension, as in the [`hooks` field](specification.md#hooks-field), and resolve it to an absolute path: a scheduled job does not inherit the shell's `PATH`.
- The model resolves: provider known, model known, credentials present, thinking level supported. Never substitute another model.
- The timeout is within the host's maximum, if it has one.

An automation that fails any of these is listed as **invalid** with the reason, and is never armed.

## Ask for consent

| State | Means | Runs |
|---|---|---|
| `invalid` | Cannot be honoured here. | No |
| `disabled` | `enabled: false`. Still listed. | No |
| `off` | Not yet armed on this machine, or changed since. | No |
| `armed` | Valid, enabled, and armed with matching content. | Yes |

An automation **must not** be armed without explicit, per-automation consent, in either scope. A repository is written by other people, and the home directory is written by agents, sync tools and installers as well as by the user, so being found there proves nothing. The preview **must** show its triggers, model, timeout, hooks and the exact command each script will run.

Pin consent to content: hash `AUTOMATION.md` and every script it can run, and return the automation to `off` when any of them changes, naming the file that changed. Re-check the pin when a run starts, not only at discovery. Hash the watch script and the hook scripts with their relative paths, and keep a hash per file so you can say which one changed. Keep consent and every other piece of state outside the repository, where a commit cannot forge it.

## Keep the clock

Something has to wake up every minute, whether or not the agent is open: for example one small job registered with the operating system (Task Scheduler on Windows, launchd on macOS, a systemd user timer or cron on Linux) that runs a tick each minute, and a way to tick in the foreground, for containers. Use one clock at a time.

On each tick, for each armed automation with a schedule:

- Evaluate the cron in the automation's `timezone`, or in the host's local zone when it has none, daylight saving included.
- Allow a grace period for a tick that comes a little late. Within it, the run is on time.
- Past the grace period, apply `catch-up`. `skip` records the instant as `missed`. `once` runs one late instance for the most recent missed instant and records the older ones as `missed`, unless the next regular instant is due within the grace period anyway. Look back a bounded window.
- Claim the instant in durable state before starting the run. A crash in between should cost at most one run, never a duplicate. Any run that starts, manual ones included, claims the instant it covers, so a manual 09:00 run stops catch-up from repeating the missed 07:00.
- Spread the load if many automations are due at the same minute, for example by delaying each by a small fixed offset derived from its identity.

Only one tick should evaluate at a time. Take a tick lock, and replace it when its owner is dead or it is too old, so a crash cannot stop the clock.

## Run watches

`on.watch` is optional; a host that does not implement it refuses automations that use it. If you implement it, follow the [watch script contract](watch-scripts.md#the-contract) exactly:

- Run the script every `every` (1 minute to 24 hours) in the automation folder, with the hook environment, no standard input and a bounded timeout.
- Fingerprint the trimmed standard output. Start a run when the script exits `0` with a fingerprint different from the one that started the last run.
- On any other exit, start nothing and forget the fingerprint. On a timeout, a start failure or a signal, start nothing and keep it.
- Do not check while a run of the same automation is going, or while the automation is not armed.
- Never pass the script's output to the run.

## Run

A run is the same whatever started it, in this order:

1. Take the run lock. One run per automation at a time. If one is going, a scheduled run is recorded as `skipped: still running since …` and a manual one is refused. Never queue behind it. Use an atomic create (`O_EXCL`) and store the owner's process id and start time, so a lock left by a dead process can be swept and recorded as `aborted`.
2. Re-check what may have changed since discovery: the consent pin, and the model.
3. Start the timeout clock. `timeout` bounds the pre hooks and the body. When it expires, stop what is running, record `failed` with `error.kind` `timeout`, and still run the post hooks.
4. Run the pre hooks in order. A script runs in the automation folder; a prompt is one turn in the run's session. A script that exits non-zero, or a prompt whose first non-empty line is `skip` or `skip: <reason>`, ends the pre hooks and records the run as `skipped`. A hook that cannot start or times out records `failed`.
5. Run the body in a fresh session, with the pre hooks' output after it in a [delimited input block](specification.md#body-content) that says it is data. Escape anything in that output that could close the block. Open the session only when it is first needed, so a run skipped by a script never starts a model.
6. Run the post hooks in order, always. A post script gets the body's answer on standard input and the outcome in `AUTOMATION_OUTCOME`. A failing post hook is recorded and never changes the outcome.
7. Write the record, then release the lock.

Every hook and watch script gets the host's environment minus credential-shaped variables (`*_API_KEY`, `*_TOKEN`, `*_SECRET`, `*_PASSWORD`, `*_CREDENTIAL(S)`), plus the [`AUTOMATION_*` variables](specification.md#environment-variables). Bound each script's run time and output.

A person must be able to stop a run. A stopped run still runs its post hooks, and the host records it as `aborted`. On Windows a signal kills the process outright and leaves its lock behind, so a stop request the run polls for (a file, for example) is safer.

## Record

Every run the host starts, and every scheduled instant it skips or misses, leaves exactly one record. A manual request refused at once, with the reason shown to the person, needs none. At minimum: the trigger (`schedule`, `manual` or `watch`), the scheduled instant, whether it was late, start and end times, the outcome and its reason or error, every hook's result, the model, and a reference to the session.

| Outcome | When |
|---|---|
| `ok` | The body answered. |
| `skipped` | A pre hook stopped it, the previous run was still going, the model did not resolve, or the consent pin no longer matches. |
| `failed` | `error.kind`: `timeout`, `model-unavailable`, `hook`, `agent` or `host`. |
| `missed` | A scheduled instant passed while the host was unavailable and `catch-up` did not run it. One record per catch-up, with a count. |
| `aborted` | Stopped by a person, or the host stopped while it ran. |

Keep records outside the repository.

## Required, recommended, optional

| | Must | Should | May |
|---|---|---|---|
| Discovery | Scan user and repository `automations/` folders | | Scan a host-specific folder too |
| Validation | Refuse unknown fields, triggers and hook kinds by name; validate at discovery | Use the one-line refusal shape | Set a maximum `timeout` |
| Consent | Keep every automation off until armed; show the preview; pin consent to a hash | | Let repository consent expire |
| Schedule | Honour `cron`, `timezone`, `catch-up` | Fire while the agent is closed | Stagger simultaneous runs |
| Watch | Refuse it if not implemented | | Implement `on.watch` |
| Run | One at a time; pre, body, post in order; post hooks always; timeout; bound every script; strip credentials | Use the `AUTOMATION_*` names | Run in an isolated worktree |
| Record | One record per run, every outcome | | Progress, notes, a message, plugins |

## List your host

If you implement the format, open an issue on [the specification's repository](https://github.com/lucas-stanford/agent-automations/issues) so the [Hosts](hosts.md) page can list your host and what it supports.
