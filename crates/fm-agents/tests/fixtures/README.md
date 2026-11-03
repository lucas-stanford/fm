# Adapter fixtures

`tests/adapters.rs` and `fm-fake-agent` (in `tests/process.rs`) replay this output of each agent
CLI, one folder per CLI. We captured the real files on Windows on 2026-10-29 in a scratch folder,
with read-only prompts and no tool-approval flags.

| CLI | file | origin |
|---|---|---|
| pi 1.2.0 | `pong.jsonl`, `tool.jsonl`, `resume.jsonl` | real (`pi -p --mode json`, prompt on stdin; `--session-dir` pointed at the scratch folder) |
| | `bad-model.stderr` | real (`--model nope/nomodel`, exit 1, empty stdout) |
| copilot 1.0.104 | `pong.jsonl`, `tool.jsonl`, `resume.jsonl` | real (`copilot --output-format json`, prompt on stdin; resume with `--resume=<id>`) |
| | `bad-model.stderr` | real (`--model nope-model`, exit 1, empty stdout) |
| claude 2.1.122 | `auth-error.jsonl` | real (installed, but the account is not usable: an API error run, exit 1) |
| | `pong.jsonl`, `tool.jsonl` | hand-written from the documented stream-json format (<https://code.claude.com/docs/en/headless>) and the real init line |
| codex 0.164.0-alpha.9 | `auth-error.jsonl` | real (not signed in: retries, then `turn.failed`, exit 1) |
| | `pong.jsonl`, `tool.jsonl` | hand-written from the documented JSONL events (<https://developers.openai.com/codex/noninteractive>, `codex-rs/exec/src/exec_events.rs`) |
| gemini 0.67.0-nightly | `no-auth.stderr` | real (not signed in: exit 41, empty stdout) |
| | `pong.jsonl`, `tool.jsonl`, `error.jsonl` | hand-written from `packages/core/src/output/types.ts` (stream-json event types) |
| opencode 1.19.4 | `pong.jsonl`, `tool.jsonl`, `resume.jsonl`, `bad-model.jsonl` | real (`opencode run --format json`, prompt on stdin; resume with `--session <id>`) |

## What we trimmed from the real captures

We removed only noise and machine details, and left every event shape as captured:

- Paths under the scratch folder now read `C:\work`, and the user name `me`.
- pi: the system message's prompt sections and tool list (they embed the user's own setup).
- copilot: `session.mcp_*`, `session.skills_loaded`, `session.tools_updated`,
  `session.usage_checkpoint` and `prompt_cache_break` lines (local MCP/skill setup), all but the
  first six `assistant.reasoning_delta` lines, and opaque fields (`reasoningOpaque`,
  `encryptedContent`, `apiCallId`, …, and any space-free string over 200 characters).
- claude: `memory_paths` from the init line.

To refresh a real fixture, run the CLI as its adapter plans it (see `docs/AGENTS.md`) in an
empty folder with the prompts `Reply with the single word: pong` and
`Read the file hello.txt with your read tool and reply with only its first word.` (with
`hello.txt` containing `banana split`), then trim as above.
