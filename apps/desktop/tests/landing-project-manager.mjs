import { readFileSync } from "node:fs";

const src = readFileSync(new URL("../src/shell/empty/Landing.tsx", import.meta.url), "utf8");

const required = [
  ["project manager shell", 'data-testid="project-manager"'],
  ["window drag strip", "data-tauri-drag-region"],
  ["recent project grid", 'data-testid="recent-project-grid"'],
  ["recent project tile", 'data-testid="recent-project-tile"'],
  ["preview media command", '"project_preview_media"'],
  ["preview state", "previewByPath"],
  ["proxy video preview", "preview?.kind === \"video\""],
  ["glass project surface", "pm-glass"],
  ["large project tiles", "minmax(280px,1fr)"],
  ["visible recent heading", "Recent Projects"],
];

for (const [label, needle] of required) {
  if (!src.includes(needle)) {
    throw new Error(`Landing project manager missing ${label}: ${needle}`);
  }
}

if (src.includes("Open a project to start editing")) {
  throw new Error("Landing still uses centered splash heading");
}

console.log(`landing-project-manager: OK (${required.length} checks)`);
