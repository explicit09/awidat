import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const shell = readFileSync(resolve(root, "src/shell/StageShell.tsx"), "utf8");
const conversation = readFileSync(resolve(root, "src/shell/StageConversation.tsx"), "utf8");
const source = `${shell}\n${conversation}`;

const checks = [
  ["renders a left-side editor pane", /stage-left-pane[\s\S]+left:\s*SIDE_PANE_GUTTER/],
  ["left pane includes transcript media and index", /LEFT_PANES[\s\S]+transcript[\s\S]+media[\s\S]+index/],
  ["renders a right-side editor pane", /stage-right-pane[\s\S]+right:\s*SIDE_PANE_GUTTER/],
  ["right pane includes chat deliver schedule inspector vedit and history", /RIGHT_PANES[\s\S]+chat[\s\S]+deliver[\s\S]+schedule[\s\S]+inspector[\s\S]+vedit[\s\S]+history/],
  ["reserves left stage space for the left pane", /paddingLeft:\s*LEFT_PANE_RESERVE/],
  ["reserves right stage space for the right pane", /paddingRight:\s*RIGHT_PANE_RESERVE/],
  ["renders the timeline unconditionally", /className="absolute bottom-6 z-20"/],
  ["offsets timeline from the left pane", /left:\s*LEFT_PANE_RESERVE/],
  ["offsets timeline from the right pane", /right:\s*RIGHT_PANE_RESERVE/],
  ["removes the vertical right tool dock", /group\/tools/.test(source) === false],
  ["removes slash destination chips from composer", /\/deliver/.test(source) === false],
  ["removes bottom composer wrapper", /absolute inset-x-0 bottom-0/.test(source) === false],
  ["keeps composer inside conversation panel", /stage-chat-composer/],
  ["does not use draggable floating chat", /setPointerCapture/.test(source) === false],
  ["does not offer a left dock control", /Dock conversation left/.test(source) === false],
  ["uses tab buttons for right-pane selection", /className="stage-right-tab"/],
  ["uses tab buttons for left-pane selection", /className="stage-left-tab"/],
];

for (const [label, pattern] of checks) {
  const ok = typeof pattern === "boolean" ? pattern : pattern.test(source);
  if (!ok) {
    throw new Error(`Stage chat layout missing: ${label}`);
  }
}

console.log(`stage-chat-layout: OK (${checks.length} checks)`);
