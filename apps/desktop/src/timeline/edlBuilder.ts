// Serialize an EDL envelope to the text format
// `crates/core/src/edl/parser.rs` consumes. Used by:
//
//   - Step 8: drag-to-trim sends a one-op TrimClip envelope
//   - Step 6: transcript-pane delete-range builds Split + Delete envelopes
//
// The format is documented at the top of parser.rs:
//
//   *** Begin EDL
//   *** Trim Clip
//   @@ anchor: clip_uuid=<uuid>
//   + start: <seconds>
//   + end: <seconds>
//   *** End EDL
//
// We emit only the `+` (set) lines — the `- ` (delta) lines are
// informational and the parser tolerates their absence. Times are
// formatted with three decimals (millisecond precision); the parser
// accepts any f64-parseable number, but we round here so the EDL
// text stays readable in the Show-EDL toggle.

export type EdlAnchor =
  | { kind: "clip_uuid"; uuid: string }
  | { kind: "transcript_snippet"; text: string };

export type EdlOp =
  | {
      kind: "trim_clip";
      anchor: EdlAnchor;
      /** Source-time start in seconds; omitted if unchanged. */
      start?: number;
      /** Source-time end in seconds; omitted if unchanged. */
      end?: number;
    }
  | { kind: "delete_clip"; anchor: EdlAnchor }
  | { kind: "split_clip"; anchor: EdlAnchor; atS: number };

/**
 * Build the canonical `*** Begin EDL` / `*** End EDL` text for one
 * envelope. Each op contributes a `***` heading + anchor + field
 * lines.
 */
export function serializeEdl(ops: EdlOp[]): string {
  const lines: string[] = [];
  lines.push("*** Begin EDL");
  for (const op of ops) {
    appendOp(lines, op);
  }
  lines.push("*** End EDL");
  return lines.join("\n") + "\n";
}

function appendOp(lines: string[], op: EdlOp): void {
  switch (op.kind) {
    case "trim_clip":
      lines.push("*** Trim Clip");
      lines.push(`@@ anchor: ${formatAnchor(op.anchor)}`);
      if (op.start !== undefined) lines.push(`+ start: ${formatTime(op.start)}`);
      if (op.end !== undefined) lines.push(`+ end: ${formatTime(op.end)}`);
      break;
    case "delete_clip":
      lines.push("*** Delete Clip");
      lines.push(`@@ anchor: ${formatAnchor(op.anchor)}`);
      break;
    case "split_clip":
      lines.push("*** Split Clip");
      lines.push(`@@ anchor: ${formatAnchor(op.anchor)}`);
      lines.push(`+ at_s: ${formatTime(op.atS)}`);
      break;
  }
}

function formatAnchor(anchor: EdlAnchor): string {
  switch (anchor.kind) {
    case "clip_uuid":
      return `clip_uuid=${anchor.uuid}`;
    case "transcript_snippet":
      // Use double-quoted form. Quotes inside the snippet are
      // unlikely (the agent's transcripts don't usually have them)
      // but we strip them defensively rather than escaping —
      // the parser doesn't define an escape grammar.
      return `transcript_snippet="${anchor.text.replace(/"/g, "")}"`;
  }
}

function formatTime(s: number): string {
  // Three decimals = millisecond precision. The parser is happy
  // with whatever f64 string Rust's str::parse accepts, but we
  // keep the EDL human-readable.
  return s.toFixed(3);
}
