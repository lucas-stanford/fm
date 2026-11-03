# Supported Mermaid

fm accepts a subset of Mermaid flowchart syntax. Anything outside it is
rejected with a `line:col` and exit status 2. The renderer,
[merman](https://github.com/Latias94/merman), also checks whatever fm accepts
before drawing it.

## Header

```
flowchart LR
graph TD
```

The direction is one of `TD`, `TB`, `BT`, `LR`, `RL`, and may be omitted.

## Nodes

| Written | Shape |
|---|---|
| `id` | none; the label is the id |
| `id[text]` | rectangle |
| `id(text)` | rounded |
| `id([text])` | stadium |
| `id[[text]]` | subroutine |
| `id[(text)]` | cylinder |
| `id((text))` | circle |
| `id>text]` | asymmetric |
| `id{text}` | rhombus |
| `id{{text}}` | hexagon |
| `id[/text/]` | parallelogram |
| `id[\text\]` | parallelogram (alt) |
| `id[/text\]` | trapezoid |
| `id[\text/]` | trapezoid (alt) |

Node ids may contain ASCII letters, digits, `-` and `_`; put punctuation in the
label. The state names nodes by id, and the SVG carries each id as `data-id`.

Labels may be quoted to contain delimiters or any Unicode:
`a["Tests [all] pass?"]`.

A node's shaped definition wins wherever it appears; a bare mention elsewhere
does not overwrite it.

## Arrows

```
a --> b          arrow
a --- b          open
a -.-> b         dotted arrow
a -.- b          dotted open
a ==> b          thick arrow
a === b          thick open
a --o b          circle end
a --x b          cross end
a -->|yes| b     labelled
a -- maybe --> b labelled
a --> b --> c    chain: two arrows
```

Whitespace around arrows is optional (`a-->b`). Cycles are fine. A `taken`
pair in the state highlights every arrow from `from` to `to`.

In text output, a flowchart merman cannot route is drawn as a rail: one
column in flow order, with each arrow that does not run to the node below
written beside its source as `──▼ <target> (<label>)`. The rail shows nodes,
arrows, arrow labels and statuses. It leaves out subgraph boxes, node shapes
and arrow styles. See the [README](../README.md#when-the-text-layout-cannot-route-a-chart).

## Subgraphs

```
flowchart TD
  subgraph prep[Prepare]
    direction LR
    a[A] --> b[B]
  end
  b --> c[C]
```

## Comments

`%%` to end of line. A `%%` inside a quoted label is kept.

## Not supported

| Construct | Why |
|---|---|
| `classDef`, `class`, `linkStyle`, `style`, `click` | fm uses them to draw statuses. |
| Mermaid diagram types other than `flowchart`/`graph` | Statuses need nodes and arrows. |
| Markdown strings, icons, `@{ shape: … }` | Not supported by fm's parser. |

## Limits

| Limit | Value | Exit status |
|---|---|---|
| Flowchart size (`--plan`) | 256 KiB | 2 |
| Nodes | 500 | 2 |
| Rendered SVG | 8 MiB | 1 |
