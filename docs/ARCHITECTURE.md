# Architecture

`fm` is a Rust host for the Agent Automations format, and its reference implementation. The
specification ([agent-automations](https://github.com/lucas-stanford/agent-automations), copied into
`tests/spec/` for the conformance tests) defines how it must behave:

| File | What it decides |
|---|---|
| `specification.md` | The format, validation order, consent, runs, records |
| `adding-support.md` | What a host must, should and may do, step by step |
| `validation.md` | Every refusal message, each with a file that produces it (55) |
| `watch-scripts.md` | The `on.watch` contract |
| `flowcharts.md` | Charts and strict flow |

Where the spec leaves a choice open, fm follows pi-automations, another host written in
TypeScript, unless this document says otherwise. `docs/GLOSSARY-pa.md` is adapted from its
`docs/GLOSSARY.md`; use its words in every message and screen.

## Independence from pi-automations

`fm` never writes pi-automations' state and reads only one file of it: pa's `trust.json`
(`$PI_AGENT_DIR/pi-automations/trust.json`, default `~/.pi/agent/pi-automations/trust.json`), to
warn that an automation armed in both hosts would run twice.

## Crates

```
fm (bin) --> fm-tui --> fm-model
   |
   +--> fm-render
   |
   +--> fm-clock --> fm-run --> fm-spec, fm-state, fm-agents --> fm-model
```

| Crate | Owns | Must not |
|---|---|---|
| `fm-render` | Mermaid flowchart parse for drawing, text and SVG rendering with node statuses (`fm render`, and a run page's chart) | know about automations |
| `fm-model` | Plain types and traits: `Automation`, `RunRecord`, `TrustFile`, `StateFile`, `Config`, `LiveEvent`, `AgentCli`, `Host`, `StatePaths` | do I/O |
| `fm-spec` | Frontmatter parse (YAML 1.2), validation in spec order with spec messages, cron grammar and arithmetic, discovery, interpreters, digests, Mermaid chart parse, `flow_order`, strict flow | write anything |
| `fm-state` | State folder resolution, `config.toml`, consent (`trust.json`), `state.json`, run locks, records (`runs/*.jsonl`), retention, stop files, the progress store | start processes |
| `fm-agents` | One `AgentCli` adapter per CLI, CLI detection, a fake CLI for tests | spawn agent turns (it only plans them) |
| `fm-run` | `fire`: lock, consent recheck, timeout, pre hooks, body, post hooks, record; script execution; hook environment; live file; progress verbs | know about the TUI |
| `fm-clock` | `tick`: schedules, grace, catch-up, claims, stagger, watches, tick lock; `serve`; the OS watcher job | |
| `fm-tui` | Every screen, against the `Host` trait; `fake::FakeHost` for tests | touch the filesystem directly |
| `fm` | clap CLI; the real `Host`; wiring | |

## State folder

`FM_HOME` if set, else the per-OS folder in the [README](../README.md#where-state-lives).
`fm_model::StatePaths` defines the layout, and the folder is mode 0700 on Unix. `fm-state` writes
every JSON file atomically (a temp file in the same folder, then a rename) under a lock, as one
read-change-write.

In `fm-state`:

- The read-change-write lock is an OS lock (`flock`/`LockFileEx`) on a sidecar `<file>.lock`, so a
  process that dies holding it releases it.
- A run lock (`locks/<slug>.lock`) is an `O_EXCL` create holding `RunLockInfo` (pid and its start
  time) plus `key`, `scheduledFor` and an optional `budgetMs`. It is stale when the owner is gone,
  its pid now belongs to a process that started at another time, or it outlived its budget; the
  sweep writes `aborted: host stopped` with the run's progress folded in.
- `locks/<slug>.stop` names the run to stop. A stop file naming another run is stale, and the
  reader clears it.
- `progress/<run id>.json` holds the run's events (`{"version":1,"events":[…]}`), folded into a
  snapshot on read; `progress/<run id>.closed` is the tombstone, swept after a week. Chart nodes
  come from `fm-spec` when the progress is opened.
- fm refuses a `config.toml` with an unknown key or a value it cannot use, whole and in one line.

## Discovery roots

fm reads only the two roots the specification names: `~/.agents/automations/` (user) and
`.agents/automations/` from the working directory up to the repository root (repository). Keys are
`<scope>:<root path>:<name>`. Arming a repository automation registers its repository root in
`state.json`, so the TUI, `fm list` and the tick find it from any folder.

## The `fm` binary

- fm finds the home directory as Node's `os.homedir()` and pi-automations do: `USERPROFILE` on
  Windows, `HOME` elsewhere.
- [CLI.md](CLI.md) defines how a name on the command line resolves and what each exit code means;
  `fm run` passes on the run's own code.
- `fm run <name>` starts `fm fire --key <key> --trigger manual --follow` and prints the run's live
  file as it grows. After `fm fire` exits, `fm run` reads the file to the end and removes it, so the
  last post hook's line is never lost. `--detach` (and the TUI's run) start the same command
  detached, without `--follow`.
- A run page's flowchart is drawn in-process by `fm-render`, from the same state JSON
  `fm render --state` reads; any failure is one line, and the TUI shows the step list instead.
- `src/seams.rs` is where `fm` calls `fm-run` (`fire`, the progress verbs) and `fm-clock` (`tick`,
  `serve`, `watcher`).
- `fm fire` ([Fire contract](#fire-contract)) prints nothing on stdout and exits 0 ok, 10 skipped,
  20 failed, 30 aborted, or 2 when refused with no record (a manual run while one is going, an
  invalid or missing automation).

## Runs (`fm-run`)

`fm_run::fire(located, FireOptions, &FireDeps) -> Result<RunRecord, FireError>` follows
`adding-support.md` "Run" step by step; the module doc of `crates/fm-run/src/fire.rs` lists the
order. Choices beyond the spec, all as pi-automations makes them unless noted:

- The lock's `budgetMs` is the timeout, plus `hook-timeout` times (post hooks + 1), plus 5
  minutes. `fire` sweeps a lock whose owner is gone (recording `aborted: host stopped`) and takes it
  at once.
- Checks after the lock, each a `skipped` record: repository gone, `not armed`, `consent expired`,
  `changed since armed: <files>`, no agent CLI set (naming `fm agent set` and `default-agent`), a
  model the CLI refuses (`error.kind` `model-unavailable`), prompt hooks on a CLI without resume,
  the CLI not found. A change leaves the automation's consent in place, so the list keeps saying
  what changed.
- The body runs in the repository root (repository scope) or the automation folder; with
  `[automations."<key>"] worktree = true`, in `git worktree add --detach <temp>/fm-wt-<run id>`,
  removed after. If fm cannot make the worktree, the run is `failed` (`host`) and does not fall
  back to the usual folder.
- Hook scripts get the scrubbed environment plus `AUTOMATION_*`; agent turns keep the login
  environment. Both get `FM_RUN`, `FM_STATE_DIR`, `FM_NODE` (when the chart has the node) and
  `fm`'s folder first on `PATH`. Post hooks get `AUTOMATION_REASON`, set to the reason, else the
  error kind.
- A polling thread reads stop requests with `fm_state::stop_requested`, which clears a stale
  request. For a request naming this run, it creates `locks/<slug>.<run id>.halt`; the process
  runner stops the pre hooks and agent turns when that file appears. Post hooks still run after a
  stop or a timeout, bounded only by `hook-timeout`.
- Agent output: every raw line goes to `sessions/<run id>.jsonl` (the record's `session.file`,
  `format` from the adapter); parsed `LiveEvent`s, and hook starts and ends, go to
  `locks/<slug>.live` (partial text throttled to one a second, everything bounded). The run removes
  the file when it ends, except under `--follow`, where the `fm run` that follows it drains and
  removes it. A failed write to the live file never fails the run.
- Progress: a run opens `progress/<run id>.json` with the chart from `fm_spec::read_chart` (no
  chart: notes and a message only) and moves `pre<i>`, `body`, `post<i>` itself; the record gets
  the steps in flow order. `fm_run::progress_cli(argv, env, now)` implements the in-run verbs, exit
  0 ok, 1 usage, 2 refused.
- A run of an automation with a schedule, manual runs included, claims the instant it covers in
  `state.json` (`scheduledFor`, else the newest cron instant at or before its start), only ever
  moving the claim forward, so catch-up does not repeat it.

## The clock

`fm-clock` runs one tick (`fm_clock::tick`) a minute, started by the watcher job, `fm serve` or
`fm tick` by hand. A tick:

1. takes the tick lock (`tick.lock`, replaced when its owner is dead or it is five minutes old);
2. sweeps abandoned run locks;
3. discovers automations in the user root and every repository root registered in `state.json`
   (`fm tick` by hand adds the working directory's);
4. decides each automation's state;
5. for each armed schedule, runs `decide_one`, the pure port of pi-automations' `decideTick`:
   grace, catch-up (`skip` writes one `missed` record with a `count`; `once` starts one late run
   for the newest instant unless the next is due within the grace period), the `catch-up-window`
   look-back, claims, and a per-key stagger. It writes the claim to `state.json` before the run
   starts. A run still going gets a `skipped` record, and a start that fails gets one `failed`
   record (`error.kind` `host`);
6. runs the due watch checks (`watch-scripts.md`) side by side, each limited to the hook timeout,
   capped at two minutes;
7. writes `lastTickAt` (rewound to just before any instant still held by its stagger) and
   `lastTickBy`.

Listings call `fm_clock::next_fire` (the next instant plus the stagger) and
`fm_clock::next_check`, so the TUI shows what the tick will do.

fm differs from pi-automations here on purpose: the look-back starts no earlier than the
automation's consent (`armedAt`). pi-automations looks back the whole window on a first tick, so
an automation armed a minute ago can get a `missed` record for instants before anyone armed it.

### Fire contract

The clock never runs a body itself. It starts this same executable, detached
(`fm_clock::DetachedSpawn`; Windows: `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP |
CREATE_NO_WINDOW`, breaking away from the tick's job object when allowed; Unix: its own process
group, null stdin), with `FM_HOME` set to the tick's state folder and output appended to
`logs/<slug>.log`:

```text
fm fire --key <automation key> --trigger <schedule|watch|manual> [--scheduled-for <iso>] [--late]
```

- `--scheduled-for` is the cron instant (not the staggered time) as `fm-state`'s ISO form,
  `2026-09-17T07:00:00.000Z`; schedule runs only. The tick has already claimed exactly this string.
- `--late` marks a `catch-up: once` run that started past its grace period.
- A watch run gets `--trigger watch` and nothing else: the script's output is never passed on.
- `--follow` (hidden) is for `fm run` only: the run leaves its live file for the follower to drain.

The watcher job runs `fm tick --by job` (`--by` is hidden; by hand it is `cli`). A quiet job tick
prints nothing, since its output is appended to `logs/tick.log`.

### The watcher

`fm watcher start|stop|status` register, remove and report one OS job per user and state folder
(`fm_clock::watcher`). Every job runs a wrapper in `<state>/watcher/` that sets an allow-list of
variables carried from the shell that ran `start` (`FM_HOME`, `PATH`, `HOME`, `LANG` and a few
more, never a credential-shaped name), then runs `fm` by its absolute path; when that binary is
gone, the wrapper removes the job instead. fm refuses to write a job file that contains a
credential's value. `watcher.json` in the state folder records what was registered; `fm serve`
refuses while it exists, and `start` refuses while `fm serve` ticked in the last two minutes.
`start` asks the scheduler for a run at once and
waits up to 90 seconds for a tick with `lastTickBy: job`.

| OS | Job | Notes |
|---|---|---|
| Windows | Task Scheduler task `agent-automations\tick` (`schtasks /Create /XML`, UTF-16): every minute, interactive token, least privilege, hidden, `IgnoreNew`, start when available | Runs `conhost.exe --headless cmd.exe /d /c <state>\watcher\fm-tick.cmd`. pi-automations hides its console with a `wscript.exe` VBScript shim; VBScript is being removed from Windows, and the console host's headless mode creates no window at all. A state folder path with `& < > ( ) @ ^ \| %` is refused (cmd.exe would read it as syntax). |
| macOS | LaunchAgent `io.agentautomations.fm.tick` in `~/Library/LaunchAgents`, `launchctl bootstrap gui/<uid>` | An empty `StartCalendarInterval` (every minute on the minute, as pi-automations), not `StartInterval 60`; `ProcessType Background`; `AbandonProcessGroup`. |
| Linux | systemd user `fm-tick.service` (oneshot, `KillMode=process` so runs outlive the tick) and `fm-tick.timer` (`OnCalendar=*-*-* *:*:00`, `Persistent=true`), `systemctl --user enable --now` | Without a systemd user session, a crontab line marked `# fm-tick (fm watcher)`. A user timer stops at logout unless `loginctl enable-linger`. |

Generators are pure and tested on every OS; scheduler commands go through `watcher::Exec`, faked in
tests. Two ignored tests cover the real Windows path. They register `fm-test-<random>` tasks
against a temporary state folder and always delete them:
`cargo test -p fm-clock --test windows_watcher -- --ignored`.

## Agent CLIs

An adapter is pure: `plan_turn` builds the command, `parse_line` turns output lines into
`LiveEvent`s, and `finish` decides the answer. `fm-run` spawns, streams, times out and kills, so the
adapter tests replay recorded output from `crates/fm-agents/tests/fixtures/<cli>/` with no CLI
installed. [AGENTS.md](AGENTS.md) has each CLI's flags, models, resume and limits.

- The consent preview shows `CommandPlan::display`, the exact command, flags included.
- Prompt hooks continue the body's session. A CLI without resume refuses prompt hooks at
  validation time: `hooks.pre[0].prompt: <cli> cannot continue a session — use a script hook, or another agent`.

The agent learns the progress commands from a block fm appends to every prompt (as pa does), with
`FM_RUN`, `FM_NODE` and `FM_STATE_DIR` in its environment: `fm step|next|say|note|message|status`.

## Conventions

- Rust 2024, stable. No `unsafe`. No async runtime: threads and `std::process`.
- Every user-facing refusal is one line, `<field path>: <what is wrong> — <what fixes it>`.
- Errors in libraries are typed (`thiserror` is fine); `anyhow` only in the `fm` binary.
- Every module has unit tests; anything that touches the clock takes `now` as a parameter.
- Paths: `std::path` only; never build paths with `/` or `\`. Tests run on all three OSes in CI.
- Cross-platform process handling: no signals for stop (Windows kills outright); the run polls its
  stop file.
- `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`
  must pass before a PR.
