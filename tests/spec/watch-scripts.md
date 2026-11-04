---
title: Writing watch scripts
nav: Watch scripts
description: How a watch script tells the host there is new work, and how to write one that starts runs only when it should.
group: For automation authors
order: 3
---

A schedule runs at fixed times whether or not there is anything to do. A watch runs a small script every few minutes, without a model, and starts a run only when the script finds something new, so an hour with no new work costs no model calls.

```markdown automation title="~/.agents/automations/pr-review/AUTOMATION.md"
---
name: pr-review
description: Review pull requests that ask me for a review, within a few minutes of the request.
timeout: 20m
on:
  watch:
    script: scripts/review_requests.mjs
    every: 5m
  manual: true
hooks:
  pre:
    - script: scripts/review_details.mjs
---

# Review requested pull requests

For each pull request in the input block, read the diff and leave one review with the
problems worth fixing, most serious first. Skip any pull request you have already reviewed
since its last push.
```

`on.watch` has two keys, both required: `script`, a path relative to the automation directory, and `every`, how often to run it, from `1m` to `24h`. For anything rarer than once a day, use [`on.schedule`](specification.md#on-schedule).

## The contract

The host runs the script and reads two things: its exit code and its standard output.

| The script | Means | The host |
|---|---|---|
| Exits `0` and prints something different from what started the last run | Found new work | Starts a run, and remembers a fingerprint of what was printed. |
| Exits `0` and prints the same as what started the last run | Same work as before | Does nothing. |
| Exits with any other code (by convention `3`) | Nothing to do | Does nothing, and forgets the last finding. |
| Times out, cannot start, or is killed by a signal | Could not check | Does nothing, and keeps the last finding. |

The fingerprint is a SHA-256 of standard output with surrounding whitespace trimmed. Nothing else about the output matters to the decision.

The script's output is **not** passed to the run; it only decides whether a run starts. To give the body the details, add a pre hook that fetches them, as `review_details.mjs` does above.

## What to print

Print one stable identifier per line, sorted: URLs, issue numbers, file names. Then the output changes exactly when the set of work changes.

```js title="scripts/review_requests.mjs"
import { execFileSync } from "node:child_process";

let urls;
try {
  const out = execFileSync("gh", ["search", "prs", "--review-requested=@me", "--state=open", "--json", "url"], {
    encoding: "utf8",
  });
  urls = JSON.parse(out).map((pr) => pr.url).sort();
} catch (e) {
  console.error(`could not ask GitHub: ${e.message.split("\n")[0]}`);
  process.exit(1);
}

if (urls.length === 0) {
  console.error("no review requests");
  process.exit(3);
}
console.log(urls.join("\n"));
```

- Leave out timestamps, counters and "checked at" lines. Anything that changes when the work has not changed starts a run on every check.
- Print only what still needs doing. When the body handles an item, the item should drop out of the output (closed, labelled, moved), so the next check sees a change or nothing at all.
- When there is nothing to do, write the reason to standard error. Listings show what the last check said: the first line of standard output after exit `0`, otherwise the last line of standard error, clipped to 200 characters.

## When checks run

- The host ticks once a minute. A watch check is due when `every` has passed since the previous one.
- No check runs while a run of the same automation is going. The next one comes after it ends.
- No check runs while the automation is not armed: invalid, `enabled: false`, or not armed on this machine yet. A watch script is code like any hook, and the consent preview shows it.
- Some hosts let you pause a watch without touching the file. No check runs while it is paused.
- A check may take at most the host's hook timeout, capped at two minutes.

## What the script gets

The script starts in the automation directory, with no standard input. Its environment is the host's, minus every credential-shaped variable, plus:

| Variable | Value |
|---|---|
| `AUTOMATION_NAME`, `AUTOMATION_KEY`, `AUTOMATION_SCOPE` | Which automation this is. |
| `AUTOMATION_DIR`, `AUTOMATION_ROOT`, `AUTOMATION_REPO_ROOT` | Where it is. `AUTOMATION_REPO_ROOT` is set for repository automations. |
| `AUTOMATION_TRIGGER`, `AUTOMATION_PHASE` | Both `watch`. |
| `AUTOMATION_LAST_CHECKED` | The ISO time of the previous check that ran to an exit code, for "what changed since". |

Scheduled checks do not read your shell profile, and variables such as `GITHUB_TOKEN` are removed. Use tools that keep their own login, such as `gh auth login`.

## State

The host remembers the fingerprint, so most watch scripts keep no state of their own. Design for two consequences:

- The host remembers the finding that started a run, whatever the run's outcome. If the run failed and the script keeps printing the same thing, no new run starts. A broken run is not retried every five minutes; run it by hand once it is fixed, or let the output change.
- Any non-zero exit forgets the finding, including an exit caused by a network error. The next successful check then counts as new even if it prints the same items, and a run starts. Write the body so that a second run over the same items does no harm.

If a script does keep its own state, it must survive being killed halfway and run again.

## Choosing a language

Scripts are started by extension, as described in the [`hooks` field](specification.md#hooks-field). `.mjs` and `.js` run on the host's own Node.js on every operating system, so use them in a repository shared across Windows, macOS and Linux. `.sh` needs bash (Git Bash on Windows), `.ps1` needs PowerShell, and `.py` needs Python on every machine that arms the automation.

## Try it

If your host can run one watch check now without starting a run, use it: it says whether a tick would start a run, and starts and records nothing. For example, [pi-automations](pi-automations.md#trying-a-watch) has such a command.

You can also run the script yourself, from the automation folder:

```sh
node scripts/review_requests.mjs; echo $?
```

Run it twice without changing anything: it should print the same lines and exit with the same code. Then make one item go away, or add one, and check that the output changes.
