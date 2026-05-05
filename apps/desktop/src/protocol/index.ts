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

// Tauri event channel names. Mirror the constants in
// `apps/desktop/src-tauri/src/lib.rs` — there is no runtime check.
export const ITEM_EVENT = "awidat://item";
export const TURN_END_EVENT = "awidat://turn-end";

// Envelope shape emitted by the backend over `ITEM_EVENT`.
import type { Item } from "./generated/Item";
export type ItemEvent = { item: Item };

// Payload emitted on TURN_END_EVENT.
export type TurnEndEvent = { error: string | null };
