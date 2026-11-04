// Pre hook: pick the one pull request this run reviews, and say where its files go.
// A `saved:` line means a run already wrote the review for this commit but did not post it.
// Exit 3 when nothing waits, which skips the run. It only reads.

import { existsSync } from "node:fs";
import path from "node:path";
import { folderFor, waiting } from "./queue.mjs";

const pr = waiting()[0];
if (!pr) {
  console.error("no pull request waits for my review");
  process.exit(3);
}
const folder = folderFor(pr);
const review = path.join(folder, "review.json");
console.log(`PICK ${pr.repo}#${pr.number} ${pr.sha} by ${pr.author}: ${pr.title}`);
console.log(`url: ${pr.url}`);
console.log(`folder: ${folder}`);
if (existsSync(review) && !existsSync(path.join(folder, "posted.json"))) console.log(`saved: ${review}`);
