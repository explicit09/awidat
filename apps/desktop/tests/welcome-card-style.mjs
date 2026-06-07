import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const source = readFileSync(join(root, "src/app/WelcomeCard.tsx"), "utf8");

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

assert(!source.includes("—"), "WelcomeCard should not use em dashes");
assert(
  source.includes("glass glass-strong"),
  "WelcomeCard dialog should use the shared strong glass shell",
);
assert(
  source.includes("glass-content"),
  "WelcomeCard idea cards should use shared glass content surfaces",
);
assert(
  source.includes("glass-cta"),
  "WelcomeCard primary action should use the shared glass CTA",
);
assert(
  source.includes("FileSearch") && source.includes("Scissors") && source.includes("CircleCheck"),
  "WelcomeCard icons should use the quieter editing-oriented icon set",
);
assert(
  source.includes("h-7 w-7") && source.includes("mt-0.5"),
  "WelcomeCard icon chips should be smaller and aligned to the title line",
);
assert(
  !source.includes("BookOpen") && !source.includes("GitBranch"),
  "WelcomeCard should not use the older mismatched welcome icons",
);

console.log("welcome-card-style: OK");
