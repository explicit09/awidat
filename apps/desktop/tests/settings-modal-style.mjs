import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const settings = readFileSync(join(root, "src/app/SettingsModal.tsx"), "utf8");
const publishing = readFileSync(join(root, "src/app/PublishingSettings.tsx"), "utf8");
const source = `${settings}\n${publishing}`;

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

assert(settings.includes("settings-shell glass glass-strong"), "Settings should use a strong glass shell");
assert(settings.includes("settings-sidebar"), "Settings should render a left section navigation");
assert(settings.includes("settings-content"), "Settings should render a dedicated content pane");
assert(settings.includes("SettingsCard"), "Settings sections should render through reusable cards");
assert(settings.includes("SettingsRow"), "Settings values should render through reusable rows");
assert(source.includes("glass-content"), "Settings and publishing surfaces should use glass content cards");
assert(source.includes("glass-cta") && source.includes("glass-ghost"), "Settings actions should use glass buttons");
assert(!settings.includes('className="modal"'), "Settings should not use the old generic modal shell");
assert(!settings.includes("modal-body"), "Settings should not use the old single-column modal body");
assert(!settings.includes("modal-footer"), "Settings should not use the old modal footer");

console.log("settings-modal-style: OK");
