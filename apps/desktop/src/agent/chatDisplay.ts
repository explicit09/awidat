import type { Item } from "../protocol";
import { isAwidatSentinel, isIntroSyntheticInput } from "../state/introState.ts";

export function visibleChatItems(items: Item[]): Item[] {
  const visible: Item[] = [];
  let hidingIntroReply = false;

  for (const item of items) {
    if (item.kind === "user_input" && isAwidatSentinel(item.text)) {
      hidingIntroReply = isIntroSyntheticInput(item.text);
      continue;
    }
    if (hidingIntroReply) {
      if (item.kind === "text") continue;
      hidingIntroReply = false;
    }
    visible.push(item);
  }

  return visible;
}
