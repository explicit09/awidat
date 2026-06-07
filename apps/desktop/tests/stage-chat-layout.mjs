import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const shell = readFileSync(resolve(root, "src/shell/StageShell.tsx"), "utf8");
const conversation = readFileSync(resolve(root, "src/shell/StageConversation.tsx"), "utf8");
const source = `${shell}\n${conversation}`;

const checks = [
  ["renders a single right-side editor pane", /stage-right-pane[\s\S]+right:\s*RIGHT_PANE_GUTTER/],
  ["right pane includes readable horizontal tabs", /RIGHT_PANES[\s\S]+chat[\s\S]+transcript[\s\S]+media[\s\S]+inspector[\s\S]+index[\s\S]+vedit/],
  ["reserves right stage space for the right pane", /paddingRight:\s*RIGHT_PANE_RESERVE/],
  ["renders the timeline unconditionally", /className="absolute inset-x-20 bottom-24 z-20"/],
  ["offsets timeline from the right pane", /right:\s*RIGHT_PANE_RESERVE/],
  ["removes the vertical right tool dock", /group\/tools/.test(source) === false],
  ["removes slash destination chips from composer", /\/deliver/.test(source) === false],
  ["does not use draggable floating chat", /setPointerCapture/.test(source) === false],
  ["does not offer a left dock control", /Dock conversation left/.test(source) === false],
  ["uses tab buttons for right-pane selection", /className="stage-right-tab"/],
];

for (const [label, pattern] of checks) {
  const ok = typeof pattern === "boolean" ? pattern : pattern.test(source);
  if (!ok) {
    throw new Error(`Stage chat layout missing: ${label}`);
  }
}

console.log(`stage-chat-layout: OK (${checks.length} checks)`);
