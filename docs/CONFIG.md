# `config.toml`

`config.toml` in the state folder holds fm's host policy, plus settings for each agent CLI and
each automation. The state folder is `FM_HOME` if set, else the per-OS folder in the
[README](../README.md#where-state-lives). Every key is optional, and with no file fm uses every
default.

fm refuses a file it cannot use as a whole and never applies part of it. An unknown key or a value
fm cannot use stops every command (exit 1) with one line, such as
`config.toml: unknown key "budget" — remove it, or use one of default-agent, grace, …`.

`fm agent default` and `fm agent set` rewrite the file with every key and its value, and drop any
comments.

## Durations

A duration is a whole number followed by `s`, `m`, `h` or `d`: `90s`, `5m`, `4h`, `7d`. Zero,
spaces and compound forms such as `1h30m` are refused.

## Keys

A test (`crates/fm/tests/docs.rs`) checks every default below against `Config::default()`.

### Host policy

| Key | Default | Meaning |
|---|---|---|
| `default-agent` | none | The agent CLI every automation runs through, unless `automations."<key>".agent` names another: `pi`, `copilot`, `claude`, `codex`, `gemini` or `opencode`. `fm agent default` sets it. With none, an automation has no agent to run its body until you set one. |
| `grace` | `"5m"` | How late a scheduled run may start and still be on time. Past it, the automation's `catch-up` applies. |
| `catch-up-window` | `"7d"` | How far back `catch-up: once` looks for a missed scheduled instant. |
| `default-timeout` | `"30m"` | The timeout of a run whose `AUTOMATION.md` has no `timeout`. |
| `max-timeout` | `"4h"` | The longest `timeout` an automation may ask for. A longer one fails the machine check and the automation is `invalid` here. |
| `hook-timeout` | `"5m"` | How long each hook may run. |
| `hook-max-output-bytes` | `65536` | How much of a hook's standard output fm keeps. fm drops the rest, and the hook's record says `truncated`. |
| `stagger-max-seconds` | `30` | The longest delay added to a scheduled run, so automations due at the same minute do not all start at once. Each automation gets a fixed delay from its automation key, at most half the time between its scheduled instants. `0` turns staggering off. |
| `consent-expiry` | `"30d"` | Consent to a repository automation expires after this long, and the automation is `off` until you arm it again. Consent to a user automation does not expire. |
| `notify-on-failed` | `true` | A desktop notification when a run fails, whatever started it. `FM_NO_NOTIFY` set in the environment also turns it off. See [CLI.md](CLI.md#notifications). |

### `[retention]`

| Key | Default | Meaning |
|---|---|---|
| `retention.max-lines-per-automation` | `2000` | When an automation's run records pass this many, the oldest are dropped. |
| `retention.keep-lines` | `1000` | How many run records are left after dropping. At least 1, and at most `max-lines-per-automation`. |

### `[agents.<cli>]`

One table per agent CLI, named `pi`, `copilot`, `claude`, `codex`, `gemini` or `opencode`.
[AGENTS.md](AGENTS.md) lists the flag each setting adds.

| Key | Default | Meaning |
|---|---|---|
| `agents.<cli>.path` | none | The CLI's executable. Set it when the CLI is not on the `PATH` the watcher sees. When absent, fm looks it up on `PATH`. |
| `agents.<cli>.extra-args` | `[]` | Arguments added after fm's own (for Codex, before `resume` and `-`). They are shown in the consent preview. |
| `agents.<cli>.allow-all-tools` | `false` | `true` adds the CLI's flag that lets the agent use tools without asking. Without it, a tool that needs approval is denied, because no one is there to approve it. |
| `agents.<cli>.default-model` | none | The model for an automation with no `model:`. A `provider/id` is mapped as `model:` is; a name without `/` is passed to the CLI as it is. When absent, the CLI's own default. |

### `[automations."<key>"]`

One table per automation, named by its full automation key in quotes (`fm check <name>` prints
it).

| Key | Default | Meaning |
|---|---|---|
| `automations."<key>".agent` | none | The agent CLI for this automation, instead of `default-agent`. `fm agent set <name> <cli>` sets it; `fm agent set <name> default` removes it. |
| `automations."<key>".worktree` | `false` | `true` runs the automation inside a temporary git worktree. |

## Every default

With no `config.toml`, fm uses:

```toml
grace = "5m"
catch-up-window = "7d"
default-timeout = "30m"
max-timeout = "4h"
hook-timeout = "5m"
hook-max-output-bytes = 65536
stagger-max-seconds = 30
consent-expiry = "30d"
notify-on-failed = true

[retention]
max-lines-per-automation = 2000
keep-lines = 1000
```

## Examples

Claude Code by default, with a longer grace period:

```toml
default-agent = "claude"
grace = "10m"

[agents.claude]
default-model = "sonnet"
```

A CLI the watcher cannot find on its `PATH`, allowed to use tools without asking:

```toml
[agents.copilot]
path = 'C:\Users\me\AppData\Local\Microsoft\WinGet\Links\copilot.exe'
allow-all-tools = true
default-model = "openai/gpt-5"
```

Codex with broader access than `allow-all-tools` gives, through `extra-args`:

```toml
[agents.codex]
extra-args = ["--sandbox", "danger-full-access"]
```

One automation through a different CLI, in a worktree:

```toml
default-agent = "copilot"

[automations."repository:/home/me/src/app/.agents/automations:manual-branch-review"]
agent = "codex"
worktree = true
```

Keep fewer run records and turn staggering off:

```toml
stagger-max-seconds = 0

[retention]
max-lines-per-automation = 200
keep-lines = 100
```
