export type ContextChip = {
  label: string;
  kind?: "media" | "selection" | "project" | "lens";
};

export type ChatSessionSummary = {
  id: string;
  title: string;
  projectRoot: string;
  logPath: string;
  startedAt: string;
  messageCount: number;
};

export type ChatHistory<TItem = unknown> = {
  session: ChatSessionSummary | null;
  items: TItem[];
};

type InvokeLike<TItem> = (
  command: string,
  args?: Record<string, unknown>,
) => Promise<ChatHistory<TItem>>;

export function buildTurnContext(chips: ContextChip[]): string[] {
  return chips
    .map((chip) => contextLine(chip))
    .filter((line): line is string => line !== null);
}

export function chatHistoryLoader<TItem>(
  invokeCommand: InvokeLike<TItem>,
  session: ChatSessionSummary,
): Promise<ChatHistory<TItem>> {
  return invokeCommand("resume_chat_session", { logPath: session.logPath });
}

function contextLine(chip: ContextChip): string | null {
  const label = chip.label.trim();
  if (!label) return null;
  switch (chip.kind) {
    case "project":
      return `project: ${stripPrefix(label, "Project:")}`;
    case "media":
      return `media: ${label}`;
    case "selection":
      return `selection: ${label}`;
    case "lens":
      return `lens: ${label}`;
    default:
      return label;
  }
}

function stripPrefix(value: string, prefix: string): string {
  return value.startsWith(prefix) ? value.slice(prefix.length).trim() : value;
}
