// Watch script: is anything waiting in inbox/? Runs every few minutes without a model.
// Exit 0 and print one file name per line when there is work. The host starts a run only when
// this output differs from what started the last run, so print stable names, never timestamps.
// Exit 3, with a reason on stderr, when there is nothing to do.

import { readdirSync } from "node:fs";
import path from "node:path";

const inbox = path.join(process.env.AUTOMATION_DIR ?? process.cwd(), "inbox");

let names = [];
try {
  names = readdirSync(inbox, { withFileTypes: true })
    .filter((entry) => entry.isFile() && !entry.name.startsWith("."))
    .map((entry) => entry.name)
    .sort();
} catch (e) {
  if (e.code !== "ENOENT") throw e;
}

if (names.length === 0) {
  console.error("inbox is empty");
  process.exit(3);
}
console.log(names.join("\n"));
