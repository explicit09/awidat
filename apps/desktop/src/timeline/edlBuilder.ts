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

import type { TimelineParameterAnimation } from "../protocol";

export type EdlAnchor =
  | { kind: "clip_uuid"; uuid: string }
  | { kind: "transcript_snippet"; text: string };

export type EdlParameterAnimation = Omit<TimelineParameterAnimation, "target"> & {
  target: TimelineParameterAnimation["target"] & {
    kind: "clip_parameter";
  };
  metadata_only?: boolean;
};

export type EdlAudioFx = {
  highPassHz?: number;
  lowPassHz?: number;
  eqBands?: Array<{ freqHz: number; gainDb: number; widthHz?: number }>;
  compressorThresholdDb?: number;
  compressorRatio?: number;
  limiterLimitDb?: number;
  noiseGateThresholdDb?: number;
  humNotchHz?: number;
  deEssHz?: number;
  deEssReductionDb?: number;
  loudnormI?: number;
  loudnormTp?: number;
};

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
  | { kind: "move_clip"; anchor: EdlAnchor; toPosition?: number; atS?: number }
  | { kind: "ripple_move"; anchor: EdlAnchor; deltaS: number }
  | { kind: "ripple_delete"; anchor: EdlAnchor }
  | {
      kind: "ripple_trim";
      anchor: EdlAnchor;
      edge: "start" | "end";
      valueS: number;
    }
  | {
      kind: "insert_transition";
      from: EdlAnchor;
      to: EdlAnchor;
      transitionId?: string;
      transitionKind?: string;
      durationS: number;
      alignment?: "center" | "start_at_cut" | "end_at_cut";
      inOffsetS?: number;
      outOffsetS?: number;
      family?: string;
      intent?: string;
      energy?: number;
      direction?: "left" | "right" | "up" | "down" | "in" | "out";
      params?: Record<string, unknown>;
    }
  | { kind: "delete_transition"; from: EdlAnchor; to: EdlAnchor }
  | {
      kind: "set_cut_intent";
      from: EdlAnchor;
      to: EdlAnchor;
      cutType: string;
      intent: string;
      audioRelation?: string;
      energy?: number;
      confidence?: number;
      reason?: string;
    }
  | { kind: "set_volume"; anchor: EdlAnchor; value: number }
  | {
      kind: "set_audio_fade";
      anchor: EdlAnchor;
      fadeInS?: number;
      fadeOutS?: number;
    }
  | {
      kind: "set_audio_lead";
      anchor: EdlAnchor;
      leadS: number;
      reason?: string;
      confidence?: number;
    }
  | {
      kind: "set_audio_trail";
      anchor: EdlAnchor;
      trailS: number;
      reason?: string;
      confidence?: number;
    }
  | {
      kind: "set_track_audio";
      track: string;
      role?: string;
      volume?: number;
      muted?: boolean;
      solo?: boolean;
    }
  | {
      kind: "set_ducking";
      track: string;
      enabled?: boolean;
      amountDb?: number;
      attackMs?: number;
      releaseMs?: number;
    }
  | {
      kind: "set_sync_group";
      anchor: EdlAnchor;
      syncGroupId: string;
      offsetS: number;
      speedFactor?: number;
      confidence?: number;
    }
  | { kind: "set_clip_audio_fx"; anchor: EdlAnchor; fx: EdlAudioFx }
  | { kind: "set_track_audio_fx"; track: string; fx: EdlAudioFx }
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
  | {
      kind: "apply_lut";
      anchor: EdlAnchor;
      lutPath: string;
      interpolation?: "nearest" | "trilinear" | "tetrahedral" | "pyramid" | "prism";
      /**
       * Blend strength in `[0, 1]`. `undefined` or `1` means the LUT
       * is applied at full intensity; lower values mix the LUT with
       * the un-graded source via FFmpeg's `blend=all_opacity` op.
       */
      strength?: number;
    }
  | { kind: "remove_lut"; anchor: EdlAnchor }
  | { kind: "set_parameter_animation"; animation: EdlParameterAnimation }
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
    case "move_clip":
      lines.push("*** Move Clip");
      lines.push(`@@ anchor: ${formatAnchor(op.anchor)}`);
      if (op.toPosition !== undefined)
        lines.push(`+ to_position: ${Math.max(0, Math.round(op.toPosition))}`);
      if (op.atS !== undefined) lines.push(`+ at_s: ${formatTime(op.atS)}`);
      break;
    case "ripple_move":
      lines.push("*** Ripple Move");
      lines.push(`@@ anchor: ${formatAnchor(op.anchor)}`);
      lines.push(`+ delta_s: ${formatTime(op.deltaS)}`);
      break;
    case "ripple_delete":
      lines.push("*** Ripple Delete");
      lines.push(`@@ anchor: ${formatAnchor(op.anchor)}`);
      break;
    case "ripple_trim":
      lines.push("*** Ripple Trim");
      lines.push(`@@ anchor: ${formatAnchor(op.anchor)}`);
      lines.push(`+ edge: ${op.edge}`);
      lines.push(`+ value_s: ${formatTime(op.valueS)}`);
      break;
    case "insert_transition":
      lines.push("*** Insert Transition");
      lines.push(`@@ between: ${formatAnchor(op.from)} and ${formatAnchor(op.to)}`);
      if (op.transitionId !== undefined) lines.push(`+ id: ${op.transitionId}`);
      lines.push(`+ kind: ${op.transitionKind ?? op.transitionId ?? "SMPTE_Dissolve"}`);
      if (op.family !== undefined) lines.push(`+ family: ${op.family}`);
      if (op.intent !== undefined) lines.push(`+ intent: ${op.intent}`);
      if (op.energy !== undefined) lines.push(`+ energy: ${formatFloat(op.energy)}`);
      if (op.direction !== undefined) lines.push(`+ direction: ${op.direction}`);
      if (op.params !== undefined)
        lines.push(`+ params_json: ${JSON.stringify(op.params)}`);
      lines.push(`+ duration_s: ${formatTime(op.durationS)}`);
      if (op.alignment !== undefined) lines.push(`+ alignment: ${op.alignment}`);
      if (op.inOffsetS !== undefined) lines.push(`+ in_offset_s: ${formatTime(op.inOffsetS)}`);
      if (op.outOffsetS !== undefined)
        lines.push(`+ out_offset_s: ${formatTime(op.outOffsetS)}`);
      break;
    case "delete_transition":
      lines.push("*** Delete Transition");
      lines.push(`@@ between: ${formatAnchor(op.from)} and ${formatAnchor(op.to)}`);
      break;
    case "set_cut_intent":
      lines.push("*** Set Cut Intent");
      lines.push(`@@ between: ${formatAnchor(op.from)} and ${formatAnchor(op.to)}`);
      lines.push(`+ cut_type: ${op.cutType}`);
      lines.push(`+ intent: ${op.intent.replace(/"/g, "")}`);
      if (op.energy !== undefined) lines.push(`+ energy: ${formatFloat(op.energy)}`);
      if (op.audioRelation !== undefined)
        lines.push(`+ audio_relation: ${op.audioRelation.replace(/"/g, "")}`);
      if (op.confidence !== undefined)
        lines.push(`+ confidence: ${formatFloat(op.confidence)}`);
      if (op.reason !== undefined) lines.push(`+ reason: "${escapeEdlText(op.reason)}"`);
      break;
    case "set_volume":
      lines.push("*** Set Volume");
      lines.push(`@@ anchor: ${formatAnchor(op.anchor)}`);
      lines.push(`+ value: ${op.value.toFixed(3)}`);
      break;
    case "set_audio_fade":
      lines.push("*** Set Audio Fade");
      lines.push(`@@ anchor: ${formatAnchor(op.anchor)}`);
      if (op.fadeInS !== undefined) lines.push(`+ fade_in_s: ${formatTime(op.fadeInS)}`);
      if (op.fadeOutS !== undefined) lines.push(`+ fade_out_s: ${formatTime(op.fadeOutS)}`);
      break;
    case "set_audio_lead":
      lines.push("*** Set Audio Lead");
      lines.push(`@@ anchor: ${formatAnchor(op.anchor)}`);
      lines.push(`+ lead_s: ${formatTime(op.leadS)}`);
      if (op.reason !== undefined) lines.push(`+ reason: "${escapeEdlText(op.reason)}"`);
      if (op.confidence !== undefined)
        lines.push(`+ confidence: ${formatFloat(op.confidence)}`);
      break;
    case "set_audio_trail":
      lines.push("*** Set Audio Trail");
      lines.push(`@@ anchor: ${formatAnchor(op.anchor)}`);
      lines.push(`+ trail_s: ${formatTime(op.trailS)}`);
      if (op.reason !== undefined) lines.push(`+ reason: "${escapeEdlText(op.reason)}"`);
      if (op.confidence !== undefined)
        lines.push(`+ confidence: ${formatFloat(op.confidence)}`);
      break;
    case "set_track_audio":
      lines.push("*** Set Track Audio");
      lines.push(`+ track: ${op.track}`);
      if (op.role !== undefined) lines.push(`+ role: ${op.role}`);
      if (op.volume !== undefined) lines.push(`+ volume: ${op.volume.toFixed(3)}`);
      if (op.muted !== undefined) lines.push(`+ muted: ${op.muted}`);
      if (op.solo !== undefined) lines.push(`+ solo: ${op.solo}`);
      break;
    case "set_ducking":
      lines.push("*** Set Ducking");
      lines.push(`+ track: ${op.track}`);
      if (op.enabled !== undefined) lines.push(`+ enabled: ${op.enabled}`);
      if (op.amountDb !== undefined) lines.push(`+ amount_db: ${op.amountDb.toFixed(3)}`);
      if (op.attackMs !== undefined) lines.push(`+ attack_ms: ${op.attackMs.toFixed(3)}`);
      if (op.releaseMs !== undefined) lines.push(`+ release_ms: ${op.releaseMs.toFixed(3)}`);
      break;
    case "set_sync_group":
      lines.push("*** Set Sync Group");
      lines.push(`@@ anchor: ${formatAnchor(op.anchor)}`);
      lines.push(`+ sync_group_id: ${op.syncGroupId.replace(/"/g, "")}`);
      lines.push(`+ offset_s: ${formatTime(op.offsetS)}`);
      if (op.speedFactor !== undefined)
        lines.push(`+ speed_factor: ${formatFloat(op.speedFactor)}`);
      if (op.confidence !== undefined)
        lines.push(`+ confidence: ${formatFloat(op.confidence)}`);
      break;
    case "set_clip_audio_fx":
      lines.push("*** Set Clip Audio FX");
      lines.push(`@@ anchor: ${formatAnchor(op.anchor)}`);
      appendAudioFx(lines, op.fx);
      break;
    case "set_track_audio_fx":
      lines.push("*** Set Track Audio FX");
      lines.push(`+ track: ${op.track}`);
      appendAudioFx(lines, op.fx);
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
      if (op.interpolation !== undefined)
        lines.push(`+ interpolation: ${op.interpolation}`);
      if (op.strength !== undefined)
        lines.push(`+ strength: ${formatFloat(op.strength)}`);
      break;
    case "remove_lut":
      lines.push("*** Remove LUT");
      lines.push(`@@ anchor: ${formatAnchor(op.anchor)}`);
      break;
    case "set_parameter_animation":
      lines.push("*** Set Parameter Animation");
      lines.push(`+ animation_json: ${JSON.stringify(op.animation)}`);
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

function appendAudioFx(lines: string[], fx: EdlAudioFx): void {
  if (fx.highPassHz !== undefined) lines.push(`+ high_pass_hz: ${formatFloat(fx.highPassHz)}`);
  if (fx.lowPassHz !== undefined) lines.push(`+ low_pass_hz: ${formatFloat(fx.lowPassHz)}`);
  if (fx.eqBands !== undefined)
    lines.push(`+ eq_bands_json: ${JSON.stringify(fx.eqBands.map((b) => ({
      freq_hz: b.freqHz,
      gain_db: b.gainDb,
      width_hz: b.widthHz,
    })))}`);
  if (fx.compressorThresholdDb !== undefined)
    lines.push(`+ compressor_threshold_db: ${formatFloat(fx.compressorThresholdDb)}`);
  if (fx.compressorRatio !== undefined)
    lines.push(`+ compressor_ratio: ${formatFloat(fx.compressorRatio)}`);
  if (fx.limiterLimitDb !== undefined)
    lines.push(`+ limiter_limit_db: ${formatFloat(fx.limiterLimitDb)}`);
  if (fx.noiseGateThresholdDb !== undefined)
    lines.push(`+ noise_gate_threshold_db: ${formatFloat(fx.noiseGateThresholdDb)}`);
  if (fx.humNotchHz !== undefined) lines.push(`+ hum_notch_hz: ${formatFloat(fx.humNotchHz)}`);
  if (fx.deEssHz !== undefined) lines.push(`+ de_ess_hz: ${formatFloat(fx.deEssHz)}`);
  if (fx.deEssReductionDb !== undefined)
    lines.push(`+ de_ess_reduction_db: ${formatFloat(fx.deEssReductionDb)}`);
  if (fx.loudnormI !== undefined) lines.push(`+ loudnorm_i: ${formatFloat(fx.loudnormI)}`);
  if (fx.loudnormTp !== undefined) lines.push(`+ loudnorm_tp: ${formatFloat(fx.loudnormTp)}`);
}

/** Strip embedded double-quotes from title text so the parser's
 *  bare quoted-string grammar doesn't trip. The parser doesn't
 *  define an escape sequence; v1 just disallows quotes inside
 *  titles. */
function escapeTitleText(s: string): string {
  return s.replace(/"/g, "");
}

function escapeEdlText(s: string): string {
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
