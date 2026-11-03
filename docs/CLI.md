# The `fm` command line

Every command and flag of `fm`, matching `fm --help` and each command's `--help`.
`crates/fm/tests/docs.rs` fails when this page misses a command.

## Naming an automation

Where a command takes `<NAME>`, give one of:

- its name, such as `watch-inbox`;
- `user/<name>` or `repository/<name>`, to pick a scope when both have one with that name;
- a prefix of its name that matches only one automation;
- its full automation key, `<scope>:<root path>:<name>`, as `fm check <name>` and `fm list --json`
  print it.

fm refuses anything else with one line (exit 2) that names the keys it could be, or says
`no automation named "x" — fm list shows them`.

## Exit codes

| Code | Means |
|---:|---|
| `0` | ok |
| `1` | usage or error: an unknown command or flag, a missing argument, a `config.toml` fm refuses, a command that could not finish |
| `2` | refused: the request was understood and declined, with one line saying why (`not armed`, `is not running`, an unknown name) |
| `10` | `fm run` only: the run was `skipped` |
| `20` | `fm run` only: the run `failed` |
| `30` | `fm run` only: the run was `aborted` |

Refusals and errors go to standard error, one line each. Answers go to standard output.

## Commands

### `fm`

```
fm
fm tui [--fake]
```

Opens the terminal UI. `fm` with no command does the same. `--fake` shows demo data without touching
real automation state; editing a demo creates a temporary sample file. The
[README](../README.md#the-terminal-ui) and the UI's own `?` page list the keys.

For `e`, the UI opens `VISUAL`, then `EDITOR`, falling back to `notepad` on Windows and `nano` then
`vi` elsewhere. [Environment variables](#environment-variables) lists the others it reads.

### `fm render`

```text
fm render --plan <file|-> [--state <file|->] [--format text|svg] [--theme dark|light] [--out <file>]
```

Draws a Mermaid flowchart with node statuses, as text or SVG, and prints it (or writes `--out`). It
keeps its own exit codes: `0` success, `1` reading, writing or rendering failed, `2` bad arguments,
flowchart or state. [RENDER.md](RENDER.md) has every option and the state JSON.

### `fm validate`

```text
fm validate <AUTOMATION.md|folder> [--name <folder name>] [--json]
```

Checks one `AUTOMATION.md` as the specification says, with no state folder, discovery or consent:
the file and its body, every field, the flowchart for `flow: strict`, and `nodes`. It does not run
the checks that need this machine (scripts runnable, the agent and model, the host's maximum
timeout); `fm check` does. `name` is checked against the file's folder, or `--name`. It prints
`<name>: valid` and exits 0, or prints the first refusal (`<field>: <what is wrong> — <what fixes
it>`) on stderr and exits 2. `--json` prints `{"ok": true, "name": …}` or `{"ok": false, "error":
…}` on stdout instead.

### `fm list`

```
fm list [--json]
```

Every automation, grouped by scope (`USER`, then each `REPOSITORY` root), with its state, its
trigger, when it next runs or checks, and the agent CLI it runs through. An automation that needs
something from you gets a second line saying what, such as `not armed — fm arm watch-inbox` or
`also armed in pa — it would run twice; pa disarm watch-inbox`.

| Flag | Does |
|---|---|
| `--json` | Prints every automation as one JSON array. See [JSON outputs](#json-outputs). |

### `fm check`

```
fm check [NAME]
```

Re-checks automations against the specification and this machine.

Without a name, it prints one line for each automation: its name, its state and, when it is not
armed, why.

With a name, it prints everything about one automation: its state and every problem, its scope,
folder, automation key, agent CLI, timeout, digest, the interpreter each script runs with, its
schedule with its stagger and next three scheduled instants, its watch and last watch check, the
run going now, the last run, whether pi-automations has it armed too, and then its consent
preview: the exact agent command, model, hooks, and the files consent is pinned to.

`fm check` exits 0 whatever the state. It exits 2 only when the name matches nothing or more than
one automation.

### `fm arm`

```
fm arm [-y | --yes] NAME
```

Prints the consent preview, then asks `Arm <name>? [y/N]`. Only `y` or `yes` arms it.

| Flag | Does |
|---|---|
| `-y`, `--yes` | Accepts the preview without asking. Needed when standard input is not a terminal, or `fm arm` refuses with `not armed — re-run with --yes to accept the preview`. |

Consent is pinned to the content hashes of `AUTOMATION.md`, every script it names, and every local
file those scripts import. fm follows a `./` or `../` path in `import … from`, `import(…)`,
`export … from` and `require(…)` in `.mjs`, `.js` and `.cjs` files, through any number of files.
Packages, and files outside the automation folder, are not pinned. Any change turns the automation
`off` until you arm it again. Consent to a repository automation also expires
after `consent-expiry` (see [CONFIG.md](CONFIG.md)). Arming a repository automation registers its
repository root, so `fm list`, the UI and the watcher find it from any folder.

fm refuses an `invalid` or `disabled` automation (exit 2). For one that is already armed, it says
so and exits 0. When pi-automations has the same automation armed, the preview starts with
`! also armed in pi-automations — it would run twice`, followed by the `pa disarm` command that
turns it off there.

### `fm disarm`

```
fm disarm NAME
```

Withdraws consent on this machine. The automation stays listed, `off`, and does not run.

### `fm enable` and `fm disable`

```
fm enable NAME
fm disable NAME
```

Set `enabled: true` or `enabled: false` in a user automation's `AUTOMATION.md`, keeping any consent
it had. They refuse a repository automation (exit 2): edit and commit its file instead, or
`fm disarm` it to keep it off on this machine.

### `fm run`

```
fm run [--detach] NAME
```

Runs one automation now, by hand (trigger `manual`). It starts `fm fire` for the automation,
prints the run's live events as they arrive (tool calls, steps, updates, the answer), then the run
record. It exits with the run's code (see [Exit codes](#exit-codes)).

| Flag | Does |
|---|---|
| `--detach` | Starts the run in the background, prints `<name> started` and returns 0. Follow it with `fm log` or in the UI. |

Refused (exit 2) when the automation is not `armed`, is already running, or has no
`manual: true` under `on:`. A run that fails shows a desktop notification (see
[Notifications](#notifications)).

### `fm stop`

```
fm stop NAME
```

Asks the run going now to stop. Its post hooks run, then it records `aborted`. Refused (exit 2)
when nothing is running.

### `fm log`

```
fm log [-n N] [--json] NAME
fm runs [-n N] [--json] NAME
```

The automation's runs, oldest of the last `N` first: when each ran, its trigger, outcome and
reason, the agent CLI, its hooks, its notes with their links, and its message. `fm runs` is the
same command.

| Flag | Does |
|---|---|
| `-n N` | How many runs. Default `10`. |
| `--json` | One run record per line (JSON Lines), exactly as stored in `runs/<name>-<hash>.jsonl` in the state folder. |

### `fm watch`

```
fm watch pause NAME
fm watch resume NAME
```

`fm watch pause` stops an automation's watch checks until `fm watch resume`. Its schedule and
manual runs still work, and the file is not changed. Refused (exit 2) for an automation with no
`on.watch`.

### `fm agent`

```
fm agent list
fm agent default CLI
fm agent set NAME CLI
```

`CLI` is one of `pi`, `copilot`, `claude`, `codex`, `gemini`, `opencode` (see
[AGENTS.md](AGENTS.md)).

| Command | Does |
|---|---|
| `fm agent list` | Every agent CLI fm knows, with the version and path found on this machine, or why it was not found. `*` marks the default. |
| `fm agent default CLI` | Sets the CLI every automation runs through unless it has its own (`default-agent` in `config.toml`). `none` clears it. |
| `fm agent set NAME CLI` | Runs one automation through `CLI` (`[automations."<key>"] agent`). `default` goes back to the default. |

An unknown CLI name exits 1. The agent CLI is part of the consent preview: changing it does not
change consent, but the next `fm check` shows the new command.

### `fm tick`

```
fm tick [--json]
```

One tick: clears run locks whose process is gone (recording `aborted`), fires every schedule that
is due, applies the grace period and `catch-up`, records missed instants, then runs the watch
checks that are due and starts a run when one finds new work. Each scheduled run starts after its
stagger. The watcher runs this once a minute; run it by hand to see what is due now.

It prints one line, such as `fired 1 · missed 0 · swept 0`, with `failed N`, `skipped N` and
`checked N` added when they are not zero, then one line for each run that could not start and
each problem. A tick that finds another tick running does nothing and says so. By hand from inside
a repository, it also reads that repository's automations, armed there or not yet registered.

| Flag | Does |
|---|---|
| `--json` | What the tick did as one JSON object. |

### `fm serve`

```
fm serve
```

Ticks once a minute in the foreground until `Ctrl-C`. Use it where you cannot register the
watcher, such as in a container. It refuses (exit 2) while the watcher is registered for this
state folder, since two clocks would then tick at once.

### `fm watcher`

```
fm watcher start [--json]
fm watcher stop [--json]
fm watcher status [--json]
```

The watcher, which fires schedules and runs watch checks, is an OS job that runs `fm tick` once a
minute for one user and one state folder. Without it or `fm serve`, no schedule fires and no watch
check runs.

| OS | Job |
|---|---|
| Windows | The Task Scheduler task `agent-automations\tick`. It runs hidden, with no console window. |
| macOS | The LaunchAgent `io.agentautomations.fm.tick` in `~/Library/LaunchAgents`. |
| Linux | The systemd user timer `fm-tick.timer`, or a crontab line marked `# fm-tick (fm watcher)` when there is no systemd user session. A user timer stops at logout unless `loginctl enable-linger` is on. |

| Command | Does |
|---|---|
| `fm watcher start` | Registers the job, asks the OS to run it at once, and waits up to 90 seconds for its first tick. Refused while `fm serve` ticked in the last two minutes. |
| `fm watcher stop` | Removes the job. |
| `fm watcher status` | Whether the watcher is `on`, `off`, `late` (registered but not ticking), or off with `fm serve` ticking instead, and when it last ticked. |

| Flag | Does |
|---|---|
| `--json` | The answer as one JSON object. |

The job carries `FM_HOME`, `PATH`, `HOME` and a few other variables from the shell that ran
`fm watcher start`, never a credential, and runs the `fm` that registered it. When that `fm` is
gone, the job removes itself. Scheduled runs do not read your shell profile: sign each agent CLI in
once, and set `[agents.<cli>] path` in `config.toml` when the CLI is not on that `PATH`.

The job appends what its ticks print to `logs/tick.log` in the state folder, and a quiet tick
prints nothing. Each run a tick starts appends its output to `logs/<name>-<hash>.log`, where the
`<hash>` tells apart automations with the same name.

## Notifications

When a run's outcome is `failed`, whatever started it, fm shows a desktop notification titled
`<name> failed`. Its text is the error or reason and how long the run took, such as
`claude exited 1: rate limited (after 2m)`. fm shows a toast through PowerShell on Windows, and uses
`osascript` on macOS and `notify-send` on Linux. If a notification cannot be shown, fm drops it;
the run and its record do not change.

`notify-on-failed = false` in `config.toml` turns notifications off. `FM_NO_NOTIFY` set to any
value turns them off for that process and every run it starts.

## Environment variables

| Variable | Does |
|---|---|
| `FM_HOME` | The state folder, instead of the per-OS one in the [README](../README.md#where-state-lives). |
| `FM_NO_NOTIFY` | Set to any value: no desktop notification when a run fails. |
| `FM_THEME` | `light` or `dark`: the UI's colours, when the terminal does not say. |
| `FM_ASCII` | `1`: the UI draws with ASCII only. |
| `NO_COLOR` | Set: the UI draws without colour. |
| `VISUAL`, `EDITOR` | The editor `e` opens in the UI. |
| `PI_AGENT_DIR` | Where pi-automations keeps its state, for the "also armed in pa" warning. Default `~/.pi/agent`. |
| `XDG_STATE_HOME` | On Linux, the parent of the state folder. |
| `AUTOMATION_RUN`, `AUTOMATION_NODE`, `AUTOMATION_STATE_DIR` | Set by fm for a run's body and hooks; see [Inside a run](#inside-a-run). |

## Inside a run

The body, every hook and everything they start get `AUTOMATION_RUN` (the run id), `AUTOMATION_STATE_DIR` and, on a
tracked run, `AUTOMATION_NODE` (`body`, `pre0`, …). With those set, these commands report progress. They
skip discovery, so they answer fast from any folder.

| Command | Does |
|---|---|
| `fm step <node> [--status active\|waiting\|done\|failed\|skipped] [-m text]` | Sets one node. `active` by default. |
| `fm next <to> [--from <node>] [-m text]` | Finishes `<node>` and starts `<to>`: a decision, and `-m` says why. `--from` defaults to `AUTOMATION_NODE`, else the one running node. In a `flow: strict` run, the move must follow an arrow of the flowchart. |
| `fm say <text> [--node <node>] [--request-id <key>]` | Adds an update to a node. |
| `fm note <text> [--link "label=https://…"]… [--request-id <key>]` | A note on the run record, with the links it published. |
| `fm message <text> [--link "label=https://…"]… [--request-id <key>]` | The run's one message for its reader. The last one wins. |
| `fm node <id> [-m text \| --file path] [--cwd dir] [--json]` | Hands a step to its own agent, as `nodes:` says (a step it does not list runs on the body's agent), and prints the agent's answer. The step gets its section, the text before the first section, what `-m` or `--file` hands over and the run's input; it runs in the body's folder or `--cwd`, within what is left of the run's timeout. Exits 0 answered, 2 refused (not a step, no section, not inside a body), 20 the agent did not answer, 30 stopped or out of time. |
| `fm status [--format markdown]` | Where the run is: every node, the arrows taken, the notes, the message. `--json` adds `history`: every step, decision (`next`, with its `-m`) and update, in order. |

Name a node by its id, or by its exact label when no other node has that label. Repeating the same
`step` or `next` changes nothing the second time; `--request-id` makes `say`, `note` and `message`
happen once however often you retry them. `-m` is short for `--message`. Every in-run command
takes `--json`, and `--run <id>` to name a run when `AUTOMATION_RUN` is not set.

They exit 0 when they made the change, 1 for a usage error (`USAGE`) or a link that is not `http`
or `https` (`BAD_LINK`), and 2 when refused: `NO_RUN`, `UNKNOWN_RUN`, `RUN_CLOSED`, `NO_CHART`,
`UNKNOWN_NODE`, `AMBIGUOUS_NODE`, `NO_ACTIVE`, `AMBIGUOUS_ACTIVE`, `NOT_AN_EDGE`, `LOCK_TIMEOUT`,
`IO`. These codes mean what they mean in pi-automations'
[reporting](https://lucas-stanford.github.io/agent-automations/reporting.html#retries-and-exit-codes).

## `fm fire`

```
fm fire --key KEY --trigger manual|schedule|watch [--scheduled-for ISO] [--late]
```

Hidden from `fm --help`. It runs one automation by its automation key, in this process, and exits
with the run's code (0, 10, 20, 30), or 2 when it refuses without writing a record. `fm run`, the
UI and the watcher start it for you. `--scheduled-for` is the scheduled instant a schedule run is
for, and `--late` marks a run that started after its grace period.

## JSON outputs

| Command | Prints |
|---|---|
| `fm list --json` | One JSON array, one object per automation: `key`, `name`, `scope` (`user`, `repository`), `dir`, `state` (`invalid`, `disabled`, `off`, `armed`), `reason`, `problems`, `automation` (the parsed frontmatter and body, `null` when invalid), `agent`, `effective_timeout_ms`, `next_fire`, `next_check`, `watch` (the last watch check), `watch_paused_at`, `running` (the run going now), `flow`, `last` (the last run's `outcome`, `at`, `reason`, `duration_ms`), `also_armed_in_pa`. Times are ISO 8601. |
| `fm log --json` | One run record per line, oldest first, as stored. Every record has `id`, `key`, `name`, `scope`, `root`, `trigger`, `late`, `recordedAt` and `outcome`; when they apply, also `scheduledFor`, `startedAt`, `endedAt`, `reason`, `error`, `agent`, `model`, `timeout`, `hooks`, `output`, `session`, `chart`, `steps`, `taken`, `notes`, `message`, `count` (instants a `missed` record covers) and `pid`. |
| `fm tick --json` | One object: `locked` (another tick was running, so this one did nothing), `considered` (armed automations with a schedule), and the automation keys it `fired`, found `missed`, `skipped` because a run was still going, `swept` (abandoned run locks), left `waiting` for their stagger, and `checked` (watch scripts run); `failed` (each a key and why it could not start) and `problems` (one line each). |
| `fm watcher status --json` | One object: `state` (`on`, `off`, `late`, `serve`), `summary` (one line), `lines` (what `fm watcher status` prints), `record` (what was registered, or `null`), `lastTickAt`, `lastTickBy` (`job`, `serve`, `cli`). |
| `fm watcher start\|stop --json` | One object: `ok`, and `lines`, what the command prints. |
| in-run commands with `--json` | One object on standard output, ok or not: `{"ok":true,"changed":true,"warnings":[],"data":{…}}`, or `{"ok":false,"error":{"code":"UNKNOWN_NODE","message":"…","suggestions":["…"]}}`. |
