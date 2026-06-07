import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const settings = readFileSync(join(root, "src/app/SettingsModal.tsx"), "utf8");
const publishing = readFileSync(join(root, "src/app/PublishingSettings.tsx"), "utf8");
const socialAccounts = readFileSync(join(root, "src/app/social/SocialAccounts.tsx"), "utf8");
const authChooser = readFileSync(join(root, "src/app/auth/AuthChooser.tsx"), "utf8");
const agentsEditor = readFileSync(join(root, "src/app/AgentsMdEditor.tsx"), "utf8");
const source = `${settings}\n${publishing}\n${socialAccounts}\n${authChooser}\n${agentsEditor}`;

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
assert(
  !settings.includes("close();\n    openAgentsMdEditor();") && !settings.includes("close();\n    openAuth();"),
  "Settings should stay open behind sub-pages so Back returns to settings",
);
assert(
  settings.includes("await openPath(path)") && settings.includes("await revealItemInDir(path)") &&
    settings.includes("SettingsError"),
  "Settings folder actions should be async, have Finder fallback, and surface failures",
);
assert(
  socialAccounts.includes("glass-cta") && socialAccounts.includes("glass-ghost") &&
    !socialAccounts.includes("from \"../../ui\""),
  "Publishing connected-account buttons should use glass controls",
);
assert(
  authChooser.includes("Back to settings") && authChooser.includes("settingsOpen") &&
    authChooser.includes("glass glass-strong") && !authChooser.includes('className="modal"'),
  "Auth chooser should be a glass settings sub-page with Back to settings",
);
assert(
  agentsEditor.includes("Back to settings") && agentsEditor.includes("settingsOpen") &&
    agentsEditor.includes("glass glass-strong") && !agentsEditor.includes('className="modal agents-md-editor"'),
  "AGENTS.md editor should be a glass settings sub-page with Back to settings",
);

console.log("settings-modal-style: OK");
