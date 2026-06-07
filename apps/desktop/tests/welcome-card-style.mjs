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

console.log("welcome-card-style: OK");
