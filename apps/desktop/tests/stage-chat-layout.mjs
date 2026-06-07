import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const shell = readFileSync(resolve(root, "src/shell/StageShell.tsx"), "utf8");
const conversation = readFileSync(resolve(root, "src/shell/StageConversation.tsx"), "utf8");
const glass = readFileSync(resolve(root, "src/ui/glass.css"), "utf8");
const source = `${shell}\n${conversation}\n${glass}`;
const rightPanesBlock = shell.match(/const RIGHT_PANES:[\s\S]+?\];/)?.[0] ?? "";

const checks = [
  ["renders a left-side editor pane", /stage-left-pane[\s\S]+left:\s*SIDE_PANE_GUTTER/],
  ["left pane starts with media then transcript then index", /LEFT_PANES[\s\S]+media[\s\S]+transcript[\s\S]+index/],
  ["renders a right-side editor pane", /stage-right-pane[\s\S]+right:\s*SIDE_PANE_GUTTER/],
  ["right pane excludes schedule and includes chat deliver inspector vedit and history", /RIGHT_PANES[\s\S]+chat[\s\S]+deliver[\s\S]+inspector[\s\S]+vedit[\s\S]+history/],
  ["schedule is not a side-pane tab", rightPanesBlock.includes("schedule") === false],
  ["side panes default wider than the first narrow pass", /const SIDE_PANE_W = 360;/],
  ["reserves left stage space for the left pane", /paddingLeft:\s*LEFT_PANE_RESERVE/],
  ["reserves right stage space for the right pane", /paddingRight:\s*RIGHT_PANE_RESERVE/],
  ["renders the timeline unconditionally", /className="absolute bottom-6 z-20"/],
  ["timeline spans full width", /left:\s*0,\s*right:\s*0/],
  ["pane heights stop above timeline", /const paneBottom = `calc\(36px \+ \$\{timelineHeight\}\)`[\s\S]+bottom:\s*paneBottom/],
  ["left pane width is stateful and resizable", /leftPaneWidth[\s\S]+setLeftPaneWidth[\s\S]+beginPaneResize\("left"/],
  ["right pane width is stateful and resizable", /rightPaneWidth[\s\S]+setRightPaneWidth[\s\S]+beginPaneResize\("right"/],
  ["timeline selection switches the stage to inspector", /useTimelineSelectionStore[\s\S]+selectedClipKey[\s\S]+setRightPane\("inspector"\)/],
  ["removes the vertical right tool dock", /group\/tools/.test(source) === false],
  ["removes slash destination chips from composer", /\/deliver/.test(source) === false],
  ["removes bottom composer wrapper", /absolute inset-x-0 bottom-0/.test(source) === false],
  ["keeps composer inside conversation panel", /stage-chat-composer/],
  ["stage composer wraps text in a textarea", /<textarea[\s\S]+className="[^"]*stage-chat-input/],
  ["stage composer accepts media suggestions", /mediaSuggestions\?:\s*MediaSuggestion\[\]/],
  ["stage composer renders the mention picker", /awidat-mention-picker/],
  ["stage composer registers picked media", /onPickMedia\?\.\(suggestion\)/],
  ["stage composer suppresses harsh focus chrome", /stage-chat-composer[\s\S]+focus-within[\s\S]+box-shadow:\s*none/],
  ["does not use the old draggable floating chat panel", /stage-chat-window/.test(source) === false],
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
