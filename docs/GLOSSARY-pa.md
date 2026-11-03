# Glossary

One name for each thing, in prose, UI text, messages and comments. The specification
(`site/content/specification.md`) wins over this file; after it, the words the `/automations` page and
`pa` show. Code identifiers, JSON fields, config keys, error codes and file names keep their names
(`trust.ts`, `flowTracking`, `NOT_AN_EDGE`, `requestCancel`, `detached.json`).

## The format

| Use | For | Not |
|---|---|---|
| Agent Automations | the format's name | agent automations |
| Draft 0.1.0 | the version | draft 0.1.0, v0.1.0, "the Draft grammar" |
| the specification | `specification.md`, in published prose | the spec (fine in contributor docs, comments and "the spec site") |
| automation | a folder with `AUTOMATION.md` and its files | job, task, workflow, recipe |
| `AUTOMATION.md` | the file, always in code font | automation.md, the AUTOMATION file |
| frontmatter | the YAML between the `---` lines | front matter, header, metadata |
| field | one of the eight top-level frontmatter keys | property, option, setting |
| key | a key inside `on.schedule`, `on.watch`, `hooks`, or in `config.json` | setting, option |
| automation key | `<scope>:<root>:<name>` | key (alone, when it could mean the above) |
| body | the Markdown after the frontmatter; the prompt for every run | instructions (as a name or heading), prompt (as a name) |
| host | a program that implements the format | runner, engine, platform |
| reference host | pi-automations, on host-neutral pages | reference implementation |

## Hosts and tools

| Use | For | Not |
|---|---|---|
| pi | the coding agent, lowercase | Pi, PI; "pi" for what pi-automations does ("pi moves", "pi's parser") |
| pi-automations | the package and host | pi automations, the extension (alone) |
| `pa` | the command line; "also installed as `pi-automations`" once per page | the CLI, `pi-automations <verb>` in messages |
| `/automations` | the slash command | /automation |
| the watcher | the OS job that ticks once a minute, firing schedules and running watch checks. Gloss it once per page: "the watcher, which fires schedules and runs watch checks". Commands: `pa watcher start\|stop\|status`, `/automations watcher …`; `detached` is a hidden alias, never documented | background job, background worker, detached mode, scheduler, daemon, ticker |
| tick | one pass of the once-a-minute clock; `pa serve` ticks in the foreground | heartbeat, cycle, ticker |
| background run | a run the watcher started, in its own process | detached run ("worker" is fine in developer docs and the plugin API) |
| Automation Manager | the bundled browser plugin, full name in user-facing text; `manager` only as the config value | AM, the manager, the edit plugin |
| plugin | a module named in `config.json` `plugins` | extension, add-on |
| state directory | `~/.pi/agent/pi-automations/` | data dir, cache |

## Scope, consent and state

| Use | For | Not |
|---|---|---|
| user scope, user automation | `~/.agents/automations/` (and `~/.pi/agent/automations/`) | personal, global, home |
| repository scope, repository automation | `.agents/automations/` up to the repository root; header `REPOSITORY`; `pa <verb> repository/<name>` (`project/` is a hidden alias) | project, repo (in prose), team, workspace |
| state | exactly one of `invalid`, `disabled`, `off`, `armed` | status, mode, on, active |
| `invalid` | a refused file or a failed machine check | broken |
| `disabled` | `enabled: false`; verbs enable and disable | turned off, paused, inactive |
| `off` | not armed here, changed since armed, or consent expired | disarmed (as a state), untrusted |
| `armed` | valid, enabled, armed, unchanged, passes machine checks; "is armed" | is on, trusted, approved |
| arm / disarm | give or withdraw consent on one machine. A person arms; the host records it | approve, trust, turn on / revoke, turn off |
| consent | what arming records, pinned to content hashes | trust, approval |
| consent preview | what `pa arm` and `a` show first; "the preview" after that | arming preview, trust prompt |
| pause / resume | a watch only (`pa watch pause`) | disable, mute |
| machine check | a check that needs the machine (script runnable, model resolves) | runtime check |
| refusal | one line: `<field path>: <what is wrong> — <what fixes it>` | error message, lint |

"healthy" may describe how the last runs went ("daily-briefing is healthy."). It is never a state.

## Triggers and runs

| Use | For | Not |
|---|---|---|
| trigger | `manual`, `schedule`, `watch` | event, condition |
| manual run, run by hand | a run a person starts | ad-hoc run |
| schedule, cron expression | `on.schedule` and its five-field string | timer, cron job, crontab |
| scheduled instant | one cron time; "fires" is fine as a verb | fire time, next fire |
| grace period | how late a run may start and still be on time | grace window |
| watch, watch script, watch check | `on.watch`, the script it runs, one run of that script. Say "watch check" when "check" could mean `pa check` | watcher (for these), poller, probe, poll. Never call `every` an interval |
| hook: pre hook, post hook, script hook, prompt hook | a `pre`/`post` item; "pre-hook" only as a modifier ("pre-hook output") | callback, step, before/after hook |
| input block | the `<automation-input>` block after the body | context block, payload |
| run | one execution | job, invocation, firing |
| outcome | exactly `ok`, `skipped`, `failed`, `missed`, `aborted` | result, status; done, succeeded, stopped, cancelled (as outcomes) |
| stop | what a person does to a running run (`x`); the outcome is `aborted`; "stop request", "stop file" | cancel, kill, abort (as the verb in prose; code may say cancel) |
| answer | the body's final reply; UI heading "Answer" | output (of the body), reply, response. Hook and watch script output stays "output" |
| run record | the one JSON line a run leaves | log entry, receipt |
| runs | an automation's recent records (`pa runs`; `pa log` is an alias) | run history, run log |

## Flowcharts and reporting

| Use | For | Not |
|---|---|---|
| flowchart; "chart" after first mention | the Mermaid `flowchart`/`graph` block in the body | map, diagram, graph, plan (except fm's `--plan`) |
| node | any box in the flowchart | box, vertex, state |
| reserved node | `pre0`…, `body`, `post0`…; the host moves them | driven node, hook node (alone) |
| step | a node the agent moves | custom step, milestone, stage |
| section | a `## Label {#id}` heading in a strict body | chapter, block |
| arrow, arrows taken | a line between two nodes; the JSON field `taken` | edge, link, transition |
| loose flow, strict flow | `flow: loose`, `flow: strict` | strict mode, flow mode |
| tracked run | a run whose progress is kept on a flowchart | charted run, trackable |
| progress | node statuses and arrows taken | flow state, trace |
| reporting | `pa step/next/say/note/message` as a whole | instrumentation, telemetry |
| update | the text `pa say` or `-m` puts on a node; "latest update" | message, comment |
| note | what `pa note` records: what the run published, with links | log line, annotation |
| message | the run's one message for its reader (`pa message`); "Last message" in the TUI | summary, report, update |
| receipts | author guidance only: "ask for receipts" = notes and the message | records |
| transcript entry | the run's entry in the pi conversation | transcript receipt |
| HTML report | `pa report` output | dashboard, export |

### Node status

`pending`, `active`, `waiting`, `done`, `failed`, `skipped`, `interrupted`. Distinct from an
automation's state and a run's outcome. What the UI shows today:

| Status | `pa status`, and the step list that stands in for a flowchart | Step tree on a run page | Automation Manager |
|---|---|---|---|
| `pending` | `·` pending | `·` | not reached |
| `active` | `▶` active | `▶` | running |
| `waiting` | `⏸` waiting | `▶` (shown as active) | waiting |
| `done` | `✓` done | `✓` | done |
| `failed` | `✗` failed | `✗` | failed |
| `skipped` | `↷` skipped | `↷` | skipped |
| `interrupted` | `‼` interrupted | `✗` (shown as failed) | interrupted |

A status the step list does not know shows as `?` unknown. The Automation Manager's "not reached"
and "running" differ from the status names; new UI should show the status name.

## UI names

| Use | For | Not |
|---|---|---|
| the `/automations` page | the full-screen TUI; sections Needs you, Running now, Coming up, Watching, Off and disabled | dashboard, attention inbox, screen, the list (for the page) |
| automation page | what `enter` opens on a row | detail view |
| check page | what `c` or `/automations check <name>` opens | Check page, details page |
| runs page | what `l` opens | log page, history |
| run page | one run: steps, flowchart, session, notes, message; same name in the TUI and the Automation Manager | run detail, run view, status window |
| Running page | what `/automations running` opens; title "Running" | Runs, status window |
| show, follow | opening a live run page (`s`, `f`, `enter`) | watch |
| History, Runs tab | Automation Manager pages | run history, run list |
| Scheduled for | the list of the next scheduled instants (check page, Automation Manager). A watch or manual run can start sooner and never appears there | Next runs, Next fires |

## Formatting

- Code font for file and folder names, fields and keys, values, commands, env vars, node ids, error
  codes, and state, outcome and node-status names when they name the value (`failed`, `armed`).
  Plain words when they read as ordinary English ("a failed run").
- UI text has no code font and uses lowercase value words ("aborted", "armed").
- Lowercase: pi, pi-automations, `pa`, fm, scope names, state and outcome words, check page, run
  page. Capitalised: Agent Automations, Automation Manager, History, Runs tab, Draft 0.1.0, Mermaid,
  YAML, Markdown, and the TUI section names.
- Headings in sentence case. TUI keys are lowercase and bare: `e`, `w`, `space`, `enter`, `esc`.
- "run id", "node id", "automation key": lowercase, two words.
- Em dashes only in one-line refusal messages and UI output (`watcher off — …`). Prose uses commas,
  colons, parentheses or a new sentence.
