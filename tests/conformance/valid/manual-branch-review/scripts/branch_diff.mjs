// Pre hook: the commits and the diff of the current branch against its base, for the body.
// Exits 3 (the run is skipped) when the branch has no changes of its own.

import { execFileSync } from "node:child_process";

const MAX_CHARS = 60_000;

const repo = process.env.AUTOMATION_REPO_ROOT;
if (!repo) {
  console.error("no repository: put this automation in <repo>/.agents/automations/");
  process.exit(2);
}

const git = (...args) => execFileSync("git", args, { cwd: repo, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 }).trim();

// The first base that exists: the remote's default branch, then main, then master.
let base;
for (const candidate of ["origin/HEAD", "origin/main", "main", "master"]) {
  try {
    git("rev-parse", "--verify", "--quiet", `${candidate}^{commit}`);
    base = candidate;
    break;
  } catch {
    // try the next one
  }
}
if (!base) {
  console.error("no base branch found: expected origin/HEAD, origin/main, main or master");
  process.exit(2);
}

const branch = git("rev-parse", "--abbrev-ref", "HEAD");
const commits = git("log", "--format=%h %s", `${base}..HEAD`);
if (commits === "") {
  console.error(`${branch} has no commits that ${base} does not have`);
  process.exit(3);
}

const diff = git("diff", `${base}...HEAD`);
console.log(`branch: ${branch}\nbase: ${base}\n\ncommits:\n${commits}\n\ndiff:`);
console.log(diff.length > MAX_CHARS ? `${diff.slice(0, MAX_CHARS)}\n[diff cut at ${MAX_CHARS} characters]` : diff);
