# Changelog

This file records every notable change. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

First public release.

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
