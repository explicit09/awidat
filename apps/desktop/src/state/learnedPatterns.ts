/**
 * learnedPatterns — pure synthesis over the proposal-history log.
 *
 * Wave 5 C4 surfaces "what the system has learned from your
 * rejections" as a Patterns panel above the History list. The agent
 * already sees these signals via the L1 context plumbing (C3); this
 * file gives the user the same view so they can sanity-check the
 * loop and decide whether to bake a pattern into AGENTS.md.
 *
 * Design choices:
 *   - **Pure helper** — input is the existing `HistoryEntry[]` slice
 *     from `useProposalHistoryStore.forProject(...)`. No new stores,
 *     no JSONL reads, no side effects. Testable in isolation.
 *   - **Heuristic suggestions** — the templates in
 *     `SUGGESTION_TEMPLATES` are hand-authored per (medium, reason)
 *     combo and crafted to read as drop-in AGENTS.md lines. No LLM
 *     call: we want determinism, instant feedback, and zero token
 *     spend on a panel users skim.
 *   - **Top-3 cap** — more than three suggestions becomes noise the
 *     user will stop reading. If a reason isn't in the templates
 *     we synthesize a generic "Avoid …" line so the panel never
 *     dead-ends, but the curated lines always win sort order.
 */
import type { BriefMedium } from "./briefProposals";
import type { HistoryEntry } from "./proposalHistory";

export interface MostRejectedMedium {
  medium: BriefMedium;
  count: number;
  /** Share of all rejections, 0-100. Rounded to nearest integer for
   *  display; raw fraction kept under `pct / 100` if callers need it. */
  pct: number;
}

export interface TopRejectReason {
  reason: string;
  count: number;
  medium: BriefMedium;
}

export interface LearnedPatterns {
  mostRejectedMedium: MostRejectedMedium | null;
  /** Highest-frequency (medium, reason) pairs, descending. Capped at 3. */
  topRejectReasons: TopRejectReason[];
  /** Share of rejects with no reason given, 0-100. 0 when no rejects. */
  silentRejectPct: number;
  /** 1-3 free-text lines suitable for paste-into-AGENTS.md. */
  suggestedAgentsMdEdits: string[];
  /** Total reject count — handy for the "N rejections logged" tile. */
  totalRejects: number;
}

/**
 * Hand-authored AGENTS.md lines keyed by `${medium}:${reason}`. Each
 * line is declarative ("Prefer …", "Avoid …") so a user can paste it
 * straight into AGENTS.md without rewriting. Templates cover the
 * preset reasons surfaced in `ProposalCard.REJECT_REASONS`; ad-hoc
 * custom reasons fall through to `genericSuggestion()`.
 *
 * Kept private — callers consume the synthesized strings, not the
 * raw map. Easier to extend without breaking the public surface.
 */
const SUGGESTION_TEMPLATES: Record<string, string> = {
  // cut
  "cut:Too aggressive":
    "Cuts: prefer narrower trims; leave breathing room around each speech segment.",
  "cut:Wrong segment":
    "Cuts: confirm the target segment against the transcript before trimming.",
  "cut:Pacing":
    "Cuts: keep pacing steady; avoid back-to-back trims that compress delivery.",
  "cut:Loses context":
    "Cuts: keep enough lead-in so the audience doesn't lose context for the line.",
  "cut:Wrong speaker":
    "Cuts: verify speaker attribution before trimming any segment.",
  // color
  "color:Too warm":
    "Color: bias the grade cooler; avoid pushing warmth on indoor footage.",
  "color:Too cool":
    "Color: bias the grade warmer; avoid pushing cyans on talking-head footage.",
  "color:Wrong skin tone":
    "Color: protect skin tones first; sample faces before applying global LUT shifts.",
  "color:Style mismatch":
    "Color: match the project's established style before introducing new looks.",
  // broll
  "broll:Off-topic":
    "B-roll: tie every generated clip back to a transcript line; reject anything off-topic.",
  "broll:Disclosure unclear":
    "B-roll: surface provider + prompt clearly on every generated clip's disclosure card.",
  "broll:Style mismatch":
    "B-roll: match the project's visual style before proposing a generated clip.",
  "broll:Wrong moment":
    "B-roll: align clip onset to the underlying line, not an arbitrary beat.",
  // audio
  "audio:Wrong levels":
    "Audio: hold dialogue around -16 LUFS; avoid heavy normalization on already-clean tracks.",
  "audio:Wrong ducking":
    "Audio: duck music tracks gently under dialogue; avoid abrupt level changes.",
  "audio:Loses ambience":
    "Audio: preserve room tone under dialogue; avoid aggressive noise reduction.",
  // caption
  "caption:Wrong word":
    "Captions: verify transcription before proposing; do not invent words on unclear audio.",
  "caption:Wrong speaker label":
    "Captions: confirm speaker labels against the transcript.",
  "caption:Timing off":
    "Captions: align caption onset to the word boundary, not the segment midpoint.",
  // transition
  "transition:Style mismatch":
    "Transitions: match the project's established style; avoid one-off effects.",
  "transition:Distracting":
    "Transitions: prefer subtle cuts; avoid flashy effects that pull focus.",
  "transition:Timing off":
    "Transitions: align onset to the cut boundary; avoid overshooting the segment.",
  // title
  "title:Style mismatch":
    "Titles: match the project's typography and color; avoid introducing new looks.",
  "title:Distracting":
    "Titles: keep titles short and quiet; avoid drawing focus away from the speaker.",
  "title:Timing off":
    "Titles: align title onset to the segment start; avoid arbitrary delays.",
};

/** Fallback when a (medium, reason) pair isn't in the curated map. */
function genericSuggestion(medium: BriefMedium, reason: string): string {
  // Display label for the medium — we keep it lowercase so the line
  // reads naturally; AGENTS.md style favors prose over Title Case.
  const label = medium === "broll" ? "B-roll" : medium === "caption" ? "Captions" : capitalize(medium);
  return `${label}: avoid "${reason}" — recent rejections flagged this pattern.`;
}

function capitalize(s: string): string {
  if (s.length === 0) return s;
  return s[0]!.toUpperCase() + s.slice(1);
}

/**
 * Synthesize the Patterns panel view-model from a per-project history
 * slice. Pure function — no I/O, no time, no globals.
 *
 * `entries` is expected to be already-project-filtered (callers pass
 * `useProposalHistoryStore.forProject(...)`). We tolerate any order
 * and any mix of decisions; non-reject rows are silently ignored
 * because the panel is reject-centric by definition.
 */
export function synthesizeLearnedPatterns(
  entries: HistoryEntry[],
): LearnedPatterns {
  const rejects = entries.filter((e) => e.decision === "rejected");
  const total = rejects.length;
  if (total === 0) {
    return {
      mostRejectedMedium: null,
      topRejectReasons: [],
      silentRejectPct: 0,
      suggestedAgentsMdEdits: [],
      totalRejects: 0,
    };
  }

  // Per-medium counts. Stable insertion-order traversal handles ties.
  const mediumCounts = new Map<BriefMedium, number>();
  for (const r of rejects) {
    mediumCounts.set(r.medium, (mediumCounts.get(r.medium) ?? 0) + 1);
  }
  let topMedium: BriefMedium | null = null;
  let topCount = 0;
  for (const [m, c] of mediumCounts) {
    if (c > topCount) {
      topMedium = m;
      topCount = c;
    }
  }
  const mostRejectedMedium: MostRejectedMedium | null =
    topMedium !== null
      ? {
          medium: topMedium,
          count: topCount,
          pct: Math.round((topCount / total) * 100),
        }
      : null;

  // (medium, reason) counts. Skip rows without a reason — those feed
  // silentRejectPct instead.
  const reasonCounts = new Map<string, TopRejectReason>();
  let silent = 0;
  for (const r of rejects) {
    const reason = (r.rejectReason ?? "").trim();
    if (reason.length === 0) {
      silent += 1;
      continue;
    }
    const key = `${r.medium}::${reason}`;
    const existing = reasonCounts.get(key);
    if (existing) {
      existing.count += 1;
    } else {
      reasonCounts.set(key, { reason, count: 1, medium: r.medium });
    }
  }
  const topRejectReasons = [...reasonCounts.values()]
    .sort((a, b) => b.count - a.count)
    .slice(0, 3);

  const silentRejectPct = Math.round((silent / total) * 100);

  // Suggestions: one line per top-reason, dedup'd. We prefer curated
  // templates and fall back to the generic line so callers always get
  // an actionable string for each visible reason.
  const seen = new Set<string>();
  const suggestedAgentsMdEdits: string[] = [];
  for (const r of topRejectReasons) {
    const key = `${r.medium}:${r.reason}`;
    const line = SUGGESTION_TEMPLATES[key] ?? genericSuggestion(r.medium, r.reason);
    if (!seen.has(line)) {
      seen.add(line);
      suggestedAgentsMdEdits.push(line);
    }
  }

  return {
    mostRejectedMedium,
    topRejectReasons,
    silentRejectPct,
    suggestedAgentsMdEdits,
    totalRejects: total,
  };
}
