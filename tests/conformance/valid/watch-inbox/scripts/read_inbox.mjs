// Pre hook: print every file waiting in inbox/ for the body, or skip the run when there is none.
// The watch script only says that something is waiting; its output never reaches the run.

import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";

const MAX_CHARS = 20_000;
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

for (const name of names) {
  const text = readFileSync(path.join(inbox, name), "utf8");
  console.log(`=== ${name} ===`);
  console.log(text.length > MAX_CHARS ? `${text.slice(0, MAX_CHARS)}\n[cut at ${MAX_CHARS} characters]` : text);
}
