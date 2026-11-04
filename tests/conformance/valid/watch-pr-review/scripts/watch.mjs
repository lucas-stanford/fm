// Watch script: does a pull request wait for my review? Runs every few minutes without a model.
// Prints one line per pull request with its latest commit, so a run starts for a new pull request
// or a new push, and not again for the same queue. Exit 3 when nothing waits.

import { waiting } from "./queue.mjs";

const queue = waiting();
if (queue.length === 0) {
  console.error("no pull request waits for my review");
  process.exit(3);
}
for (const pr of queue) console.log(`${pr.repo}#${pr.number} ${pr.sha.slice(0, 7)} ${pr.author}: ${pr.title}`);
