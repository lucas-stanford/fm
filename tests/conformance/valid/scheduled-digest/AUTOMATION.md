---
name: scheduled-digest
description: Every weekday at 08:00 UTC, summarise the commits made in this repository since the previous working day.
timeout: 10m
on:
  schedule:
    cron: "0 8 * * MON-FRI"
    timezone: UTC
  manual: true
hooks:
  pre:
    - script: scripts/recent_commits.mjs
---

# Commit digest

1. Read the commits in the input block below. Each line is `<hash> <author> <subject>`.
2. Group them by area of the code, using the subjects and your knowledge of this repository.
3. Write one line per group: the area, what changed, and the short hashes.
   Put anything that looks risky (a revert, a migration, a security fix) first.
4. Keep the whole digest under twenty lines. Do not change any files.

If you post the digest anywhere, record it with its link. Finish with one short message for the reader:
the one line that matters most today, with the digest's link if you posted it.
