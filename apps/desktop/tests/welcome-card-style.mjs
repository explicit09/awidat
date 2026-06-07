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
assert(!source.includes("lucide-react"), "WelcomeCard idea rows should not use pictogram icons");
assert(
  source.includes("step: \"01\"") && source.includes("step: \"02\"") && source.includes("step: \"03\""),
  "WelcomeCard idea rows should use quiet numbered steps",
);
assert(
  source.includes("border-l border-[rgba(239,68,68,0.42)]") &&
    source.includes("font-mono text-[10px]"),
  "WelcomeCard idea rows should use a small editorial rail instead of icon chips",
);

console.log("welcome-card-style: OK");
