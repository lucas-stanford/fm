---
name: watch-inbox
description: When new text files appear in this automation's inbox folder, summarise each one with its action items.
timeout: 15m
on:
  watch:
    script: scripts/watch.mjs
    every: 5m
  manual: true
hooks:
  pre:
    - script: scripts/read_inbox.mjs
---

# Summarise the inbox

The input block below holds every file waiting in `inbox/`, each under a `=== <file name> ===` line.
This run starts in the automation folder, so the paths below are relative to it.

For each file:

1. If `summaries/<file name>.md` already exists, skip to step 4: it was handled by an earlier run.
2. Write `summaries/<file name>.md`: a three-line summary, then the action items as a list,
   each with an owner when the notes name one.
3. Do not change the original file's contents.
4. Move the original into `inbox/processed/`, creating that folder if needed.

Finish with one line per file: its name and how many action items it had. Record that you
summarised them, then leave one short message for the reader: how many files, and the action
items that need them.
