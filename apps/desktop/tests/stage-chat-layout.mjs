import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const shell = readFileSync(resolve(root, "src/shell/StageShell.tsx"), "utf8");
const conversation = readFileSync(resolve(root, "src/shell/StageConversation.tsx"), "utf8");
const timelinePane = readFileSync(resolve(root, "src/timeline/TimelinePane.tsx"), "utf8");
const app = readFileSync(resolve(root, "src/App.tsx"), "utf8");
const inspector = readFileSync(resolve(root, "src/inspector/ClipInspector.tsx"), "utf8");
const emptyConversation = readFileSync(resolve(root, "src/agent/EmptyConversation.tsx"), "utf8");
const glass = readFileSync(resolve(root, "src/ui/glass.css"), "utf8");
const appCss = readFileSync(resolve(root, "src/App.css"), "utf8");
const source = `${shell}\n${conversation}\n${timelinePane}\n${app}\n${inspector}\n${emptyConversation}\n${glass}\n${appCss}`;
const stageSource = `${shell}\n${conversation}`;
const rightPanesBlock = shell.match(/const RIGHT_PANES:[\s\S]+?\];/)?.[0] ?? "";

const checks = [
  ["renders a left-side editor pane", /stage-left-pane[\s\S]+left:\s*SIDE_PANE_GUTTER/],
  ["left pane starts with media then transcript then index", /LEFT_PANES[\s\S]+media[\s\S]+transcript[\s\S]+index/],
  ["renders a right-side editor pane", /stage-right-pane[\s\S]+right:\s*SIDE_PANE_GUTTER/],
  ["right pane excludes history and includes chat deliver inspector vedit", /RIGHT_PANES[\s\S]+chat[\s\S]+deliver[\s\S]+inspector[\s\S]+vedit/],
  ["history is not a side-pane tab", rightPanesBlock.includes("history") === false],
  ["schedule is not a side-pane tab", rightPanesBlock.includes("schedule") === false],
  ["side panes default wider than the first narrow pass", /const SIDE_PANE_W = 360;/],
  ["reserves left stage space for the left pane", /paddingLeft:\s*LEFT_PANE_RESERVE/],
  ["reserves right stage space for the right pane", /paddingRight:\s*RIGHT_PANE_RESERVE/],
  ["renders the timeline unconditionally", /className="absolute bottom-6 z-20"/],
  ["timeline spans full width", /left:\s*0,\s*right:\s*0/],
  ["stage keeps timeline toolbar visible", /stage-timeline \.timeline-header \{ display: none/.test(glass) === false],
  ["stage timeline toolbar uses glass styling", /\.stage-timeline \.timeline-header[\s\S]+backdrop-filter:\s*blur/],
  ["timeline toolbar shows pixels-per-second readout", /px\/s/],
  ["timeline toolbar has an explicit fit button", />\s*Fit\s*</],
  ["timeline header separates left transport and zoom regions", /timeline-header-left[\s\S]+timeline-header-center[\s\S]+timeline-header-right/],
  ["timeline transport is centered by CSS grid", /\.timeline-header[\s\S]+grid-template-columns:\s*minmax\(0,\s*1fr\) auto minmax\(0,\s*1fr\)/],
  ["timeline header layers above the timeline stage", /\.timeline-header[\s\S]+position:\s*relative[\s\S]+z-index:\s*40/],
  ["track add menu layers above timeline content", /\.timeline-add-track[\s\S]+z-index:\s*45[\s\S]+\.timeline-add-track-menu[\s\S]+z-index:\s*60/],
  ["timeline stage stays below toolbar menus", /\.timeline-stage[\s\S]+position:\s*relative[\s\S]+z-index:\s*0/],
  ["timeline toolbar exposes playback controls", /TimelineTransportControls[\s\S]+aria-label=\{isPlaying \? "Pause timeline" : "Play timeline"\}/],
  ["timeline toolbar exposes jump controls", /aria-label="Jump to start"[\s\S]+aria-label="Jump to end"/],
  ["timeline toolbar exposes playback speed presets", /setPreviewRate[\s\S]+1\.5[\s\S]+2/],
  ["pane heights stop above timeline", /const paneBottom = `calc\(36px \+ \$\{timelineHeight\}\)`[\s\S]+bottom:\s*paneBottom/],
  ["stage timeline height is user resizable", /timelineHeightPx[\s\S]+setTimelineHeightPx[\s\S]+beginTimelineResize/],
  ["stage timeline defaults to at least two visible tracks", /const TL_MIN_VISIBLE_TRACKS = 2[\s\S]+Math\.max\(TL_MIN_VISIBLE_TRACKS, tracks\)/],
  ["stage timeline sizing includes real header ruler and lane heights", /const TL_HEADER_PX = 40[\s\S]+const TL_RULER_PX = 22[\s\S]+const TL_ROW = 62/],
  ["stage timeline resize handle stays on the top edge", /stage-timeline-resize-handle/],
  ["left pane width is stateful and resizable", /leftPaneWidth[\s\S]+setLeftPaneWidth[\s\S]+beginPaneResize\("left"/],
  ["right pane width is stateful and resizable", /rightPaneWidth[\s\S]+setRightPaneWidth[\s\S]+beginPaneResize\("right"/],
  ["timeline selection switches the stage to inspector", /useTimelineSelectionStore[\s\S]+selectedClipKey[\s\S]+setRightPane\("inspector"\)/],
  ["removes the vertical right tool dock", /group\/tools/.test(source) === false],
  ["removes slash destination chips from composer", /\/deliver/.test(stageSource) === false],
  ["removes bottom composer wrapper", /absolute inset-x-0 bottom-0/.test(stageSource) === false],
  ["keeps composer inside conversation panel", /stage-chat-composer/],
  ["stage chat exposes visible session controls", /stage-chat-session-header[\s\S]+Chat history[\s\S]+New chat/],
  ["stage chat lists saved sessions", /chatSessions\.map\(\(session\)[\s\S]+onSelectChatSession\?\.\(session\)/],
  ["stage shell passes chat sessions into stage chat", /chatSessions=\{chatSessions\}[\s\S]+onSelectChatSession=\{onSelectChatSession\}/],
  ["app wires stage chat session handlers", /chatSessions=\{chatSessions\}[\s\S]+onSelectChatSession=\{\(session\) => void selectChatSession\(session\)\}[\s\S]+onNewChat=\{\(\) => void startNewChat\(\)\}/],
  ["stage composer wraps text in a textarea", /<textarea[\s\S]+className="[^"]*stage-chat-input/],
  ["stage composer uses a fixed action well", /stage-chat-action-well[\s\S]+onSubmit/],
  ["stage composer aligns the send control", /\.stage-chat-action-well[\s\S]+align-items:\s*center/],
  ["stage composer accepts media suggestions", /mediaSuggestions\?:\s*MediaSuggestion\[\]/],
  ["stage composer renders the mention picker", /awidat-mention-picker/],
  ["stage composer registers picked media", /onPickMedia\?\.\(suggestion\)/],
  ["stage chat body owns mouse wheel scrolling", /className="stage-chat-scroll min-h-0 flex-1 overflow-auto"/],
  ["stage chat scroll contains wheel overscroll", /\.stage-chat-scroll[\s\S]+overscroll-behavior:\s*contain/],
  ["stage composer suppresses harsh focus chrome", /stage-chat-composer[\s\S]+focus-within[\s\S]+box-shadow:\s*none/],
  ["does not use the old draggable floating chat panel", /stage-chat-window/.test(source) === false],
  ["does not render the agent read line under the preview", /agentRead \? \([\s\S]+color-brand-hover[\s\S]+\) : null/.test(shell) === false],
  ["does not offer a left dock control", /Dock conversation left/.test(source) === false],
  ["uses tab buttons for right-pane selection", /className="stage-right-tab"/],
  ["uses tab buttons for left-pane selection", /className="stage-left-tab"/],
  ["inspector does not show the publish bridge", /EditorPublishBridge/.test(inspector) === false],
  ["stage media grid shows source media visually", /stage-media-grid grid grid-cols-2/],
  ["stage media cards render thumbnails when available", /item\.thumbnail[\s\S]+<img[\s\S]+src=\{item\.thumbnail\}/],
  ["stage media cards include thumbnail fallbacks", /mediaInitials\(item\.title\)/],
  ["stage media cards know the selected media", /const selectedMediaStem = useMediaStore\(\(s\) => s\.selectedStem\)/],
  ["stage media cards expose selected state", /data-selected=\{item\.stem === selectedMediaStem \? "true" : "false"\}/],
  ["stage media selected state uses visible contrast", /\.stage-media-item\[data-selected="true"\][\s\S]+rgba\(239,68,68,0\.22\)/],
  ["stage media thumbnails use dedicated glass styling", /\.stage-media-thumb[\s\S]+aspect-ratio:\s*16 \/ 9/],
  ["stage media empty state spans the grid", /\.stage-media-empty[\s\S]+grid-column:\s*1 \/ -1/],
  ["source preview selector has liquid-visible contrast", /\.media-asset-select[\s\S]+rgba\(10,\s*10,\s*18,\s*0\.86\)/],
  ["empty conversation does not promise to work only after indexing", /I'll work on it once indexing finishes/.test(emptyConversation) === false],
];

for (const [label, pattern] of checks) {
  const ok = typeof pattern === "boolean" ? pattern : pattern.test(source);
  if (!ok) {
    throw new Error(`Stage chat layout missing: ${label}`);
  }
}

console.log(`stage-chat-layout: OK (${checks.length} checks)`);
