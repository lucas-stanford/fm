// Pre hook: list the commits since the previous working day, or skip the run when there are none.
// Exit 0 with output: the run goes ahead and the body reads this output as data.
// Any other exit: the run is recorded as skipped, with the last stderr line as the reason.

import { execFileSync } from "node:child_process";

// Monday's run also covers the weekend.
const since = new Date().getUTCDay() === 1 ? "72 hours ago" : "24 hours ago";

// Repository automations get AUTOMATION_REPO_ROOT. A user automation has none.
const repo = process.env.AUTOMATION_REPO_ROOT;
if (!repo) {
  console.error("no repository: put this automation in <repo>/.agents/automations/");
  process.exit(2);
}

let log;
try {
  log = execFileSync("git", ["log", `--since=${since}`, "--no-merges", "--format=%h %an %s"], {
    cwd: repo,
    encoding: "utf8",
  });
} catch (e) {
  console.error(`git log failed: ${String(e.message).split("\n")[0]}`);
  process.exit(1);
}

const commits = log.trim();
if (commits === "") {
  console.error(`no commits since ${since}`);
  process.exit(3);
}
console.log(commits);
