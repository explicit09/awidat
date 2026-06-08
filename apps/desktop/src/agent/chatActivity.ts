import type { Item } from "../protocol";

export type ActivityItem = Extract<Item, { kind: "tool_call" | "job" }>;

export type ChatDisplayEntry =
  | { kind: "item"; item: Item }
  | { kind: "activity_group"; id: string; items: ActivityItem[] };

export function groupActivityItems(items: Item[]): ChatDisplayEntry[] {
  const out: ChatDisplayEntry[] = [];
  let current: ActivityItem[] = [];

  function flush() {
    if (current.length === 0) return;
    out.push({
      kind: "activity_group",
      id: `activity-${current[0].id}-${current[current.length - 1].id}`,
      items: current,
    });
    current = [];
  }

  for (const item of items) {
    if (isActivityItem(item)) {
      current.push(item);
      continue;
    }
    flush();
    out.push({ kind: "item", item });
  }
  flush();
  return out;
}

export function activityGroupLabel(items: ActivityItem[]): string {
  const jobs = items.filter((item) => item.kind === "job").length;
  const tools = items.length - jobs;
  const parts: string[] = [];
  if (jobs > 0) parts.push(`${jobs} ${jobs === 1 ? "job" : "jobs"}`);
  if (tools > 0) parts.push(`${tools} ${tools === 1 ? "tool call" : "tool calls"}`);
  return parts.length > 0 ? parts.join(" · ") : "Agent activity";
}

export function activityStatus(
  item: ActivityItem,
): "running" | "done" | "failed" | "cancelled" {
  if (item.kind === "tool_call") {
    if (item.result === null || item.phase !== "completed") return "running";
    return "Err" in item.result ? "failed" : "done";
  }
  if (item.phase !== "completed" || item.result === null) return "running";
  if (item.result === "cancelled") return "cancelled";
  return "err" in item.result ? "failed" : "done";
}

export function jobResultText(item: Extract<Item, { kind: "job" }>): string {
  if (item.result === null) return "Running";
  if (item.result === "cancelled") return "Cancelled";
  if ("ok" in item.result) return item.result.ok.summary ?? "Done";
  return item.result.err.message;
}

export function jobLabel(kind: Extract<Item, { kind: "job" }>["job_kind"]): string {
  const labels: Record<Extract<Item, { kind: "job" }>["job_kind"], string> = {
    url_import: "url import",
    local_import: "local import",
    transcode: "transcode",
    thumbnails: "thumbnails",
    waveform: "waveform",
    silences: "silences",
    motion: "motion",
    indexing: "indexing",
    generated_media: "generated media",
    render: "export",
  };
  return labels[kind];
}

function isActivityItem(item: Item): item is ActivityItem {
  return item.kind === "tool_call" || item.kind === "job";
}
