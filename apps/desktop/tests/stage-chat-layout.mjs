import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const shell = readFileSync(resolve(root, "src/shell/StageShell.tsx"), "utf8");
const conversation = readFileSync(resolve(root, "src/shell/StageConversation.tsx"), "utf8");
const source = `${shell}\n${conversation}`;

const checks = [
  ["renders chat as a right-side pane", /stage-chat-pane[\s\S]+right:\s*chatRight/],
  ["reserves right stage space for open chat", /paddingRight:\s*rightReserve/],
  ["renders the timeline unconditionally", /className="absolute inset-x-20 bottom-24 z-20"/],
  ["offsets timeline from the right chat pane", /right:\s*rightReserve/],
  ["does not use draggable floating chat", /setPointerCapture/.test(source) === false],
  ["does not offer a left dock control", /Dock conversation left/.test(source) === false],
  ["uses icons for chat controls", /PanelRight[\s\S]+Minimize2/],
  ["keeps chat control buttons square", /className="stage-chat-icon"/],
];

for (const [label, pattern] of checks) {
  const ok = typeof pattern === "boolean" ? pattern : pattern.test(source);
  if (!ok) {
    throw new Error(`Stage chat layout missing: ${label}`);
  }
}

console.log(`stage-chat-layout: OK (${checks.length} checks)`);
