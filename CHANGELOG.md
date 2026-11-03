# Changelog

This file records every notable change. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

First public release.

- fm runs Agent Automations. The host that was `aat` in the agent-automations repository moves
  here and becomes `fm`: discovery and validation of `AUTOMATION.md` exactly as the specification
  says, consent, schedules, watches, the OS watcher, runs through pi, GitHub Copilot CLI, Claude
  Code, Codex CLI, Gemini CLI or OpenCode, run records, progress on a flowchart, and the terminal
  UI (`fm` with no command). Its environment variables are `FM_*` (`FM_HOME`, `FM_RUN`, …), its
  watcher job is `io.agentautomations.fm.tick` on macOS, and its state folder is unchanged.
- A run page draws the run's flowchart in-process, so `fm-path` in `config.toml` and `FM_BIN` are
  gone.
- fm is a Cargo workspace: the renderer is the `fm-render` library, the host is `fm-model`,
  `fm-spec`, `fm-state`, `fm-agents`, `fm-run`, `fm-clock` and `fm-tui`, and the `flow-manager`
  package builds the `fm` binary.
- The terminal UI uses ratatui 0.30.
- `tests/spec/` holds the specification's pages; the conformance tests run every example in them.

- `fm render` draws a Mermaid flowchart with a status on each node, as plain
  text (no escape codes) or as a standalone, sanitised SVG.
- Node statuses and arrows taken come from an optional JSON state: `pending`,
  `active`, `waiting`, `done`, `failed`, `skipped`, `interrupted`.
- Dark (default) and light themes for SVG; every node carries `data-id`.
- A documented Mermaid flowchart subset, with `line:col` errors for anything
  outside it.
- Exit status 2 for bad input, 1 for I/O or render failures.
- When merman's text layout cannot route a flowchart, text output falls back to
  a one-column rail with every arrow written beside its source, instead of
  failing with `routes crossing reserved graph geometry` or `unroutable graph
  edges`.
- Install scripts for prebuilt Linux, macOS and Windows release binaries.
