// Pure detection helpers behind the stage insights panel. Everything
// here is computed from data the app already has (whisper transcript
// words, silence sidecars) — no invented numbers. Kept free of
// React/stores so it's unit-testable in node.

export type DetectedMoment = {
  kind: "silence" | "filler";
  /** Stem of the source/proxy the moment was detected in. */
  stem: string;
  /** Source-media seconds where the moment starts. */
  sourceTimeS: number;
  /** How much material the moment covers, in seconds. */
  durationS: number;
  /** Short row title (e.g. `Silence (4.2s)`). */
  label: string;
  /** Muted detail (e.g. the filler text or context). */
  detail: string;
};

/** Singles counted as filler when they stand alone. Deliberately
 *  conservative — no bare "like"/"so", which are usually real words. */
const FILLER_SINGLES = new Set(["um", "uh", "uhm", "uhh", "umm", "erm", "ehm"]);
/** Two-word verbal tics, matched across consecutive words. */
const FILLER_BIGRAMS = new Set(["you know", "i mean", "kind of", "sort of"]);

type WordLike = { text: string; start_s: number; end_s: number };

export function detectFillerMoments(
  words: readonly WordLike[],
  stem: string,
): DetectedMoment[] {
  const moments: DetectedMoment[] = [];
  for (let i = 0; i < words.length; i += 1) {
    const word = normalizeWord(words[i].text);
    if (FILLER_SINGLES.has(word)) {
      moments.push(fillerMoment(words[i], words[i], stem, words[i].text));
      continue;
    }
    const next = words[i + 1];
    if (next && FILLER_BIGRAMS.has(`${word} ${normalizeWord(next.text)}`)) {
      moments.push(fillerMoment(words[i], next, stem, `${words[i].text} ${next.text}`));
      i += 1;
    }
  }
  return moments;
}

export function detectSilenceMoments(
  silences: readonly { start_s: number; duration_s: number }[],
  stem: string,
  minDurationS: number,
): DetectedMoment[] {
  return silences
    .filter((s) => s.duration_s >= minDurationS)
    .map((s) => ({
      kind: "silence" as const,
      stem,
      sourceTimeS: s.start_s,
      durationS: s.duration_s,
      label: `Silence (${s.duration_s.toFixed(1)}s)`,
      detail: "Dead air",
    }));
}

/** Seconds an edit pass could remove. Silences keep a natural beat
 *  (`keepS`) per cut; filler removal reclaims the full word span. */
export function estimatedSavingsS(moments: readonly DetectedMoment[], keepS = 0.3): number {
  return moments.reduce(
    (sum, m) => sum + Math.max(0, m.durationS - (m.kind === "silence" ? keepS : 0)),
    0,
  );
}

function fillerMoment(
  first: WordLike,
  last: WordLike,
  stem: string,
  text: string,
): DetectedMoment {
  return {
    kind: "filler",
    stem,
    sourceTimeS: first.start_s,
    durationS: Math.max(0, last.end_s - first.start_s),
    label: "Filler word",
    detail: `“${text.trim()}”`,
  };
}

function normalizeWord(text: string): string {
  return text.toLowerCase().replace(/[^a-z']/g, "");
}
