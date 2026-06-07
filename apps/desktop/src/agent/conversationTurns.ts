import type { Item } from "../protocol";
import type { ConversationTurn } from "../shell/CommandRail";
import { isMontageSentinel } from "../state/introState.ts";

type ToolCallItem = Extract<Item, { kind: "tool_call" }>;

export function itemsToConversationTurns(
  items: Item[],
  summarizeTool: (item: ToolCallItem) => string,
): ConversationTurn[] {
  const out: ConversationTurn[] = [];
  let current: ConversationTurn | null = null;

  for (const it of items) {
    if (it.kind === "user_input") {
      // Synthetic editorial turns (intro F3, prepare B4) are hidden
      // from the transcript. Open a headerless turn so the agent's
      // response still has a home but no user bubble is drawn for
      // the editorial instruction.
      if (isMontageSentinel(it.text)) {
        current = {
          id: `sentinel-${it.id.toString()}`,
          userText: "",
          parts: [],
        };
        out.push(current);
        continue;
      }
      current = {
        id: it.id.toString(),
        userText: it.text,
        parts: [],
      };
      out.push(current);
      continue;
    }

    if (!current) {
      current = { id: `pre-${it.id}`, userText: "", parts: [] };
      out.push(current);
    }

    if (it.kind === "text") {
      const text = it.text.trim();
      if (text.length === 0) continue;
      current.parts.push({ kind: "text", id: it.id.toString(), text });
    } else if (it.kind === "tool_call") {
      const status = !it.result
        ? "running"
        : "Err" in it.result
          ? "failed"
          : "done";
      current.parts.push({
        kind: "tool_call",
        id: it.id.toString(),
        name: it.name,
        status,
        summary: summarizeTool(it),
        args: it.args ?? null,
        result: it.result,
      });
    } else if (it.kind === "awaiting_user_input") {
      current.parts.push({
        kind: "input_request",
        id: it.id.toString(),
        question: it.question,
        options: it.options ?? null,
      });
    } else if (it.kind === "approval_request") {
      current.parts.push({
        kind: "approval_request",
        id: it.id.toString(),
        toolName: it.tool_name,
        argsSummary: it.args_summary,
        capabilityMetadata: it.capability_metadata,
      });
    }
  }

  return out.slice(-12);
}
