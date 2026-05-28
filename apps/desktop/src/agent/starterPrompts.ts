// Project-aware starter prompts shown in the empty agent rail. Each
// project type maps to four high-signal opening moves; unknown or
// missing types fall back to a generic set. Pure data + lookup so the
// table is unit-testable without React.

const PROMPTS_BY_TYPE: Record<string, string[]> = {
  podcast: [
    "Trim filler & long silences",
    "Cut to the punchline (≤ 90s)",
    "Find a YouTube-ready highlight",
    "Apply podcast cleanup defaults",
  ],
  interview: [
    "Remove cross-talk & dead air",
    "Pull the strongest 3 quotes",
    "Make a 60-second teaser",
    "Apply interview cleanup defaults",
  ],
  highlight: [
    "Find the biggest moments",
    "Cut a 30-second teaser",
    "Tighten reaction beats",
    "Apply highlight defaults",
  ],
};

const FALLBACK: string[] = [
  "Trim long silences",
  "Summarize what's in this clip",
  "Suggest a starting edit",
  "Show me the loudest moments",
];

export function getStarterPrompts(projectType: string | undefined | null): string[] {
  if (!projectType) return FALLBACK;
  return PROMPTS_BY_TYPE[projectType] ?? FALLBACK;
}
