# Drawing flowcharts: `fm render`

`fm render` draws a [Mermaid](https://mermaid.js.org/) flowchart with the status of each node: done,
active, failed and so on. You give it the chart and a small JSON state; it prints plain text for a
terminal or a standalone SVG for a browser. It runs nothing and keeps nothing between calls, so any
tool that tracks progress through a flowchart can call it. fm's own run pages draw a run's chart the
same way, in-process.

```mermaid
flowchart LR
  start[Start] --> check{Looks right?}
  check -->|yes| ship[Ship it]
  check -->|no| fix[Fix it]
  check -- unsure --> ask[Ask a human]
```

```console
$ fm render --plan branch.mmd --state state.json
┌─────────┐     ╭────────────────╮        ┌───────────────┐
│         │     ╭                ╮  yes   │               │
│ Start ✓ ├────►< Looks right? ✓ ├────┬──►│   Ship it ▶   │
│         │     ╰                ╯    │   │               │
└─────────┘     ╰────────┬───────╯    │   └───────────────┘
                         │   no       │
                         └────────────┤
                                      │
                                      │   ┌───────────────┐
                                unsure├──►│    Fix it ↷   │
                                      │   └───────────────┘
                                      │
                                      │   ┌───────────────┐
                                      └──►│ Ask a human · │
                                          └───────────────┘
```

`--format svg` draws the same flowchart with each node filled in its status
colour and the arrows taken highlighted, in a dark or light theme.

## Screenshots

A release pipeline, mid-run, in the terminal:

![fm render showing a release flowchart in the terminal, with done, skipped, active and pending steps](../assets/screenshots/terminal.png)

The same plan and state with `--format svg`:

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../assets/screenshots/release-dark.svg">
  <img src="../assets/screenshots/release-light.svg" width="360" alt="fm SVG flowchart of the release pipeline, each step coloured by its status">
</picture>

The chart and state are in [`assets/screenshots`](../assets/screenshots).


## Usage

```
fm render --plan <file|-> [--state <file|->] [--format text|svg] [--theme dark|light] [--out <file>]
```

| Option | Meaning |
|---|---|
| `--plan` | Mermaid flowchart to draw; `-` reads stdin. Required. |
| `--state` | Node statuses and arrows taken (JSON); `-` reads stdin. Optional. |
| `--format` | `text` (default): box-drawing text with no escape codes. `svg`: a standalone, sanitised SVG. |
| `--theme` | `dark` (default) or `light`. Applies to SVG. |
| `--out` | Write to a file instead of stdout. |

Only one of `--plan` and `--state` can read stdin. In SVG output every node
group carries `data-id="<nodeId>"`, so a page can find a node by its id.

### When the text layout cannot route a chart

merman's text layout cannot yet route every flowchart. A chart where many
arrows cross, such as several steps that can each stop early and jump to the
same final step, is one example. For such a chart, `--format text` draws a
rail instead of failing. The nodes run down one column in flow order,
whatever direction the chart asks for. An arrow to the node drawn below runs
between the boxes, and every other arrow is written beside its source and
points up (`▲`) or down (`▼`) to its target, or round (`↺`) to the node itself:

```text
┌────────────────────┐
│      Build ✓       │
└──────────┬─────────┘
           ┃
           ▼
┌────────────────────┐
│      Check ▶       │──▲ Build (first failure)
│                    │──▼ Write the report (second failure)
└──────────┬─────────┘
           │
           ▼
┌────────────────────┐
│      Review ·      │──▼ Write the report (no review)
└──────────┬─────────┘
```

That is part of [`fixtures/plans/exits.mmd`](../fixtures/plans/exits.mmd), mid-run.
A taken arrow is drawn heavy (`┃`, `━━▲`). SVG output is never affected.

## State

```json
{
  "nodes": {"start": "done", "check": "done", "ship": "active", "fix": "skipped"},
  "taken": [["start", "check"], ["check", "ship"]]
}
```

- `nodes` maps a node id to a status. A node that is not listed is `pending`.
- `taken` lists arrows, as `[from, to]` pairs, to highlight.

Both fields are optional, and fm ignores unknown fields. A node id, status or
arrow that the flowchart does not have is an error.

| Status | Glyph |
|---|---|
| `pending` | `·` |
| `active` | `▶` |
| `waiting` | `⏸` |
| `done` | `✓` |
| `failed` | `✗` |
| `skipped` | `↷` |
| `interrupted` | `‼` |

## Supported Mermaid

A flowchart subset: `flowchart`/`graph` with a direction, the common node
shapes, eight arrow types, arrow labels, chains, subgraphs, cycles and `%%`
comments. fm rejects styling directives (`classDef`, `class`, `style`,
`linkStyle`, `click`) because it uses them to draw statuses, and reports
anything else it does not support with its `line:col`.
[docs/SUPPORTED_MERMAID.md](SUPPORTED_MERMAID.md) has the details.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Success. |
| `1` | Reading, writing or rendering failed. |
| `2` | Bad arguments, flowchart or state. Retrying with the same input fails the same way. |

stdout carries only the rendering; errors go to stderr as `fm: <message>`.
