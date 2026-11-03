# Agent CLIs

`crates/fm-agents` has one adapter per agent CLI; it plans the command and parses the output, and
`fm-run` spawns the process. We checked the flags on 2026-10-29 against what the version column
lists and the docs each adapter's module comment links.

| CLI | version checked | invocation (prompt on stdin) | JSON stream? | resume | tool-approval flag (`allow-all-tools = true`) | model mapping (`model:` to argument) | transcript format | known limitations |
|---|---|---|---|---|---|---|---|---|
| pi | 1.2.0, `--help` + real runs | `pi -p --mode json [--model P/ID] [--thinking L] [--session ID]` | yes: `session`, `message_update`, `message_end`, `tool_execution_start`, `agent_end` | yes, `--session <id>` | none: pi never asks | any `provider/id` as is; `:L` becomes `--thinking L` (off, minimal, low, medium, high, xhigh, max) | `pi/json` | pi needs Node 22.19 or later (its own shim picks one) |
| GitHub Copilot CLI | 1.0.104 (`--help` + real runs; smoke-tested on 1.0.105) | `copilot --output-format json [--model ID] [--reasoning-effort L] [--allow-all-tools] [--resume=ID]` | yes: `assistant.message_delta`, `assistant.reasoning_delta`, `assistant.message`, `tool.execution_start`, `result` | yes, `--resume=<id>` | `--allow-all-tools` | `github-copilot/ID`, `copilot/ID`, and `anthropic/`, `openai/`, `google/`, `xai/` become `--model ID`; `:L` becomes `--reasoning-effort` (none to max); other providers refused | `copilot/jsonl` | session id only arrives in the final `result` line; an admin policy can replace the model (`session.info`); without `--allow-all-tools`, tools needing approval are denied |
| Claude Code | 2.1.122, `--help` + a real (auth-error) run | `claude -p --output-format stream-json --verbose [--model ID] [--effort L] [--dangerously-skip-permissions] [--resume ID]` | yes: `system`/`init`, `assistant`, `user`, `result` (+ `stream_event` with `--include-partial-messages`) | yes, `--resume <id>` | `--dangerously-skip-permissions` | `anthropic/ID` becomes `--model ID`; `:L` becomes `--effort` (low, medium, high, xhigh, max); other providers refused | `claude/stream-json` | no partial text unless `--include-partial-messages` is in `extra-args`; success fixtures hand-written (no signed-in account here) |
| Codex CLI | 0.164.0-alpha.9 `--help` + a real unauthenticated run; docs for 0.165.0 | `codex exec --json --skip-git-repo-check [-m ID] [-c model_reasoning_effort=L] [--sandbox workspace-write] [extra] [resume ID] -` | yes: `thread.started`, `item.*`, `turn.completed`, `turn.failed`, `error` | yes, `exec resume <id> -` | `--sandbox workspace-write` (exec never prompts; default sandbox is read-only) | `openai/ID` becomes `-m ID`; `:L` becomes `-c model_reasoning_effort=L` (any word; Codex validates per model); other providers refused | `codex/exec-json` | `extra-args` go before `resume`/`-`; broader access (`--sandbox danger-full-access`, `--yolo`) only via `extra-args` with `allow-all-tools` off; retry notices arrive as `error` events (cleared when the turn completes) |
| Gemini CLI | 0.67.0-nightly `--help` + a real no-auth run; docs for 0.66.0 | `gemini --output-format stream-json [-m ID] [--approval-mode yolo] [--resume ID]` | yes: `init`, `message`, `tool_use`, `tool_result`, `error`, `result` | yes, `--resume <session id>` | `--approval-mode yolo` | `google/ID` becomes `-m ID`; `:L` refused (no thinking flag); other providers refused | `gemini/stream-json` | headless because stdin is piped (not a TTY); `--help` documents `--resume` as `latest`/index, the CLI reference also by session id; stream fixtures hand-written (no signed-in account here) |
| OpenCode | 1.19.4, `--help` + real runs; source at v1.19.5 | `opencode run --format json [-m P/ID] [--variant L] [--auto] [--session ID]` | yes: `step_start`, `text`, `reasoning`, `tool_use`, `step_finish`, `error` | yes, `--session <id>` | `--auto` | any `provider/id` as is; `:L` becomes `--variant L` (any word; provider-specific) | `opencode/json` | tool calls are reported when they finish, not when they start; reasoning only with `--thinking` in `extra-args`; no partial text (text parts arrive finished) |

## Common rules

- Every CLI above reads the prompt from stdin (`PromptVia::Stdin`), which avoids quoting and the
  Windows command-line limit of about 32K characters. `CommandPlan::display` shows the prompt as
  `< <prompt>`.
- fm adds the table's tool-approval flag only when `[agents.<cli>] allow-all-tools = true`.
- `extra-args` come after fm's own flags (for Codex, before `resume`/`-`), and the consent preview
  shows them.
- fm refuses a model the CLI cannot serve, never substituting one:
  `model: <what is wrong> — <what fixes it>`. With no `model:`, fm uses `default-model` from
  config, else the CLI's own default (`<cli> default`). A `default-model` without `/` goes to the
  CLI as its own model name, such as `sonnet`.
- The answer is the CLI's final text (`Final`), else the last assistant text. A non-zero exit with
  no answer is an error carrying a bounded, one-line excerpt of stderr; an error the CLI reports in
  its stream wins over that.
- `Partial` and `Thinking` live events keep the last 2000 characters, `Message` the first 8000. A
  tool summary is one line of at most 120 characters: the path, command, URL or query the tool
  acted on. Lines that are not JSON objects produce nothing, and parsing never panics.

## Finding the CLI (`detect`)

fm uses `[agents.<cli>] path` if set, else searches PATH. On Windows the search tries each
`PATHEXT` extension (default `.COM;.EXE;.BAT;.CMD`), then `.PS1`, and skips the extension-less `sh`
script npm writes beside its shims. Detection runs `<cli> --version` with a 15 s timeout and kills
only the process it started. A version it cannot read is `None`, not an error.

### Windows shims

fm spawns `.exe`, `.cmd` and `.bat` files directly. Rust's `std::process::Command` runs batch files
through `cmd.exe /d /c` with cmd-safe quoting, and refuses an argument it cannot quote safely
(tested with `&`, spaces and quotes through a `.cmd` shim). A `.ps1` with no `.cmd` beside it runs
as `powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File <script> …`.

fm runs the shim, not the node script behind it: shim layouts differ between npm, pnpm, volta and
pi's own shim (which picks a newer node), and the shim is what you run. Killing a `.cmd` shim kills
`cmd.exe` but not the node process under it, so `fm-run` stops the whole process tree.

## The fake agent

`fm-fake-agent` (`crates/fm-agents/src/bin/fm-fake-agent.rs`, logic in `fm_agents::fake`) stands
in for any CLI in tests. Point an adapter at it with `path`, and pass its options through
`extra-args`, which every adapter appends:

```toml
[agents.claude]
path = "target/debug/fm-fake-agent"        # .exe on Windows
extra-args = ["--fm-fake-fixture=crates/fm-agents/tests/fixtures/claude/pong.jsonl"]
```

It ignores every other argument, reads stdin to the end, replays the fixture to stdout line by
line and exits. Each option falls back to an environment variable when absent:

| Option | Variable | Does |
|---|---|---|
| `--fm-fake-fixture=<path>` | `FM_FAKE_FIXTURE` | fixture to replay |
| `--fm-fake-exit=<code>` | `FM_FAKE_EXIT` | exit code |
| `--fm-fake-stderr=<text>` | `FM_FAKE_STDERR` | written to stderr before exiting |
| `--fm-fake-delay-ms=<n>` | `FM_FAKE_DELAY_MS` | sleep before each stdout line |
| `--fm-fake-hang` | `FM_FAKE_HANG=1` | sleep an hour after replaying, for timeouts |
| `--fm-fake-record=<path>` | `FM_FAKE_RECORD` | write the arguments, a `--- stdin ---` line, then stdin |

`--version` prints `fm-fake-agent <version>`.

In `fm-agents` tests, `env!("CARGO_BIN_EXE_fm-fake-agent")` gives its path. Cargo does not set
that variable for a dependency's binary, so another crate either adds a one-line binary of its own,
`fn main() { std::process::exit(fm_agents::fake::main()) }`, or looks for `fm-fake-agent` next to
its test executable after `cargo build -p fm-agents`.

## Checking against a real CLI

`real_cli_smoke` in `crates/fm-agents/tests/process.rs` costs a model call, so it is ignored by
default. For each CLI in `FM_AGENTS_REAL`, it detects the CLI, runs a pong turn from Rust and
parses the output:

```sh
FM_AGENTS_REAL=pi,copilot,opencode cargo test -p fm-agents --test process -- --ignored real_ --nocapture
```

It passed on 2026-10-29 on Windows for pi (via `pi.cmd`), copilot (`.exe`) and opencode (via
`opencode.cmd`).
