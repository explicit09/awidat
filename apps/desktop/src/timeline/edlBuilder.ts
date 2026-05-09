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
  | { kind: "split_clip"; anchor: EdlAnchor; atS: number }
  | { kind: "set_volume"; anchor: EdlAnchor; value: number }
  | { kind: "set_speed"; anchor: EdlAnchor; factor: number }
  | {
      kind: "set_color_correction";
      anchor: EdlAnchor;
      exposureEv?: number;
      contrast?: number;
      saturation?: number;
      temperature?: number;
      tint?: number;
      shadows?: number;
      highlights?: number;
    }
  | { kind: "apply_lut"; anchor: EdlAnchor; lutPath: string }
  | {
      kind: "insert_title";
      startS: number;
      endS: number;
      text: string;
      position?: "top" | "center" | "bottom";
      fontSize?: number;
      color?: string;
      fontWeight?: "normal" | "bold";
      animation?:
        | "none"
        | "fade_in"
        | "fade_out"
        | "fade_in_out"
        | "slide_in"
        | "slide_out";
    }
  | {
      kind: "set_title";
      anchor: EdlAnchor;
      startS?: number;
      endS?: number;
      text?: string;
      position?: "top" | "center" | "bottom";
      fontSize?: number;
      color?: string;
      fontWeight?: "normal" | "bold";
      animation?:
        | "none"
        | "fade_in"
        | "fade_out"
        | "fade_in_out"
        | "slide_in"
        | "slide_out";
    };

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
    case "set_volume":
      lines.push("*** Set Volume");
      lines.push(`@@ anchor: ${formatAnchor(op.anchor)}`);
      lines.push(`+ value: ${op.value.toFixed(3)}`);
      break;
    case "set_speed":
      lines.push("*** Set Speed");
      lines.push(`@@ anchor: ${formatAnchor(op.anchor)}`);
      lines.push(`+ factor: ${op.factor.toFixed(3)}`);
      break;
    case "set_color_correction":
      lines.push("*** Set Color Correction");
      lines.push(`@@ anchor: ${formatAnchor(op.anchor)}`);
      if (op.exposureEv !== undefined)
        lines.push(`+ exposure_ev: ${formatFloat(op.exposureEv)}`);
      if (op.contrast !== undefined)
        lines.push(`+ contrast: ${formatFloat(op.contrast)}`);
      if (op.saturation !== undefined)
        lines.push(`+ saturation: ${formatFloat(op.saturation)}`);
      if (op.temperature !== undefined)
        lines.push(`+ temperature: ${formatFloat(op.temperature)}`);
      if (op.tint !== undefined) lines.push(`+ tint: ${formatFloat(op.tint)}`);
      if (op.shadows !== undefined)
        lines.push(`+ shadows: ${formatFloat(op.shadows)}`);
      if (op.highlights !== undefined)
        lines.push(`+ highlights: ${formatFloat(op.highlights)}`);
      break;
    case "apply_lut":
      lines.push("*** Apply LUT");
      lines.push(`@@ anchor: ${formatAnchor(op.anchor)}`);
      lines.push(`+ lut_path: ${op.lutPath.replace(/"/g, "")}`);
      break;
    case "insert_title":
      lines.push("*** Insert Title");
      lines.push(`+ start_s: ${formatTime(op.startS)}`);
      lines.push(`+ end_s: ${formatTime(op.endS)}`);
      lines.push(`+ text: "${escapeTitleText(op.text)}"`);
      if (op.position !== undefined) lines.push(`+ position: ${op.position}`);
      if (op.fontSize !== undefined) lines.push(`+ font_size: ${op.fontSize}`);
      if (op.color !== undefined) lines.push(`+ color: ${op.color}`);
      if (op.fontWeight !== undefined)
        lines.push(`+ font_weight: ${op.fontWeight}`);
      if (op.animation !== undefined)
        lines.push(`+ animation: ${op.animation}`);
      break;
    case "set_title":
      lines.push("*** Set Title");
      lines.push(`@@ anchor: ${formatAnchor(op.anchor)}`);
      if (op.startS !== undefined) lines.push(`+ start_s: ${formatTime(op.startS)}`);
      if (op.endS !== undefined) lines.push(`+ end_s: ${formatTime(op.endS)}`);
      if (op.text !== undefined)
        lines.push(`+ text: "${escapeTitleText(op.text)}"`);
      if (op.position !== undefined) lines.push(`+ position: ${op.position}`);
      if (op.fontSize !== undefined) lines.push(`+ font_size: ${op.fontSize}`);
      if (op.color !== undefined) lines.push(`+ color: ${op.color}`);
      if (op.fontWeight !== undefined)
        lines.push(`+ font_weight: ${op.fontWeight}`);
      if (op.animation !== undefined)
        lines.push(`+ animation: ${op.animation}`);
      break;
  }
}

/** Strip embedded double-quotes from title text so the parser's
 *  bare quoted-string grammar doesn't trip. The parser doesn't
 *  define an escape sequence; v1 just disallows quotes inside
 *  titles. */
function escapeTitleText(s: string): string {
  return s.replace(/"/g, "");
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

function formatFloat(value: number): string {
  return value.toFixed(3);
}
