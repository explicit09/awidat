// Re-exports the auto-generated wire types from `./generated/`.
//
// Frontend code should `import { Item, Turn, ... } from "../protocol"`
// rather than reaching into `./generated/` directly. Anything we add
// by hand (event channel name constants, helper guards) lives here
// so the generated dir stays a pure ts-rs output.

export type { Id } from "./generated/Id";
export type { Item } from "./generated/Item";
export type { ItemLifecycle } from "./generated/ItemLifecycle";
export type { JobKind } from "./generated/JobKind";
export type { JobResult } from "./generated/JobResult";
export type { PlanStep } from "./generated/PlanStep";
export type { Turn } from "./generated/Turn";
export type { Thread } from "./generated/Thread";
// Timeline shapes — single source of truth for both `read_timeline`
// responses and the `Item::ProposedEdit.snapshot` field.
export type { TimelineItem } from "./generated/TimelineItem";
export type { TimelineTrack } from "./generated/TimelineTrack";
export type { TimelineSnapshot } from "./generated/TimelineSnapshot";
export type { TrackAudioControls } from "./generated/TrackAudioControls";
export type { DuckingControls } from "./generated/DuckingControls";
export type { ColorCorrectionStyling } from "./generated/ColorCorrectionStyling";
export type { TitleStyling } from "./generated/TitleStyling";
export type { BroadcastOverlayConfig } from "./generated/BroadcastOverlayConfig";
export type { BroadcastHost } from "./generated/BroadcastHost";
export type { BroadcastTimedEntry } from "./generated/BroadcastTimedEntry";
export type { BroadcastOverlayStyle } from "./generated/BroadcastOverlayStyle";
// Approval-as-diff additions.
export type { ProposalSource } from "./generated/ProposalSource";
export type { AppliedDiff } from "./generated/AppliedDiff";
export type { Side } from "./generated/Side";
export type { EditAdjustment } from "./generated/EditAdjustment";
export type { AdjustField } from "./generated/AdjustField";
// Transcript pane (Step 6).
export type { Transcript } from "./generated/Transcript";
export type { TranscriptSegment } from "./generated/TranscriptSegment";
export type { TranscriptWord } from "./generated/TranscriptWord";
export type { TranscriptSpeaker } from "./generated/TranscriptSpeaker";
// Editorial perception (Phase 1).
export type { EditorialNoteKind } from "./generated/EditorialNoteKind";
export type { EditorialNoteStatus } from "./generated/EditorialNoteStatus";
export type { ProjectType } from "./generated/ProjectType";
export type { PermissionMode } from "./generated/PermissionMode";
// Continuity engine (Phase 2).
export type { ContinuityVerdictTag } from "./generated/ContinuityVerdictTag";
// B-roll suggestions (Phase 3).
export type { BrollPreview } from "./generated/BrollPreview";

// Tauri event channel names. Mirror the constants in
// `apps/desktop/src-tauri/src/lib.rs` — there is no runtime check.
export const ITEM_EVENT = "awidat://item";
export const TURN_END_EVENT = "awidat://turn-end";
export const TIMELINE_CHANGED_EVENT = "awidat://timeline-changed";
export const MENU_COMMAND_EVENT = "awidat://menu-command";

// Envelope shape emitted by the backend over `ITEM_EVENT`.
import type { Item } from "./generated/Item";
export type ItemEvent = { item: Item };

// Payload emitted on TURN_END_EVENT.
export type TurnEndEvent = { error: string | null };

// Payload emitted on MENU_COMMAND_EVENT.
export type NativeMenuCommandEvent = { id: string };
