// The queue both scripts share: open pull requests that ask for my review, each with its latest
// commit. It only reads, through the GitHub CLI (`gh auth login` once; set GH_HOST for GitHub
// Enterprise). A new pull request or a new push changes the queue.

import { execFileSync } from "node:child_process";
import os from "node:os";
import path from "node:path";

function gh(args) {
  return JSON.parse(execFileSync("gh", args, { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }));
}

/** Waiting pull requests, oldest number first: { repo, number, sha, author, title, url }. */
export function waiting() {
  const found = gh([
    "search", "prs", "--review-requested=@me", "--state=open", "--draft=false",
    "--json", "number,repository,title,author,url", "--limit", "30",
  ]);
  return found
    .map((pr) => {
      const repo = pr.repository.nameWithOwner;
      const { headRefOid } = gh(["pr", "view", String(pr.number), "-R", repo, "--json", "headRefOid"]);
      return { repo, number: pr.number, sha: headRefOid, author: pr.author.login, title: pr.title, url: pr.url };
    })
    .sort((a, b) => a.repo.localeCompare(b.repo) || a.number - b.number);
}

/** Where a run keeps its files for one commit of one pull request. Never the automation's folder. */
export function folderFor(pr) {
  return path.join(os.tmpdir(), "watch-pr-review", `${pr.repo.replace("/", "-")}-${pr.number}-${pr.sha.slice(0, 7)}`);
}
