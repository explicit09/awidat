export function shouldReplaceDeferredChatHistory(args: {
  scheduledProject: string | null;
  currentProject: string | null;
  scheduledItemCount: number;
  currentItemCount: number;
  running: boolean;
}): boolean {
  return (
    args.scheduledProject === args.currentProject &&
    !args.running &&
    args.scheduledItemCount === args.currentItemCount
  );
}

export function shouldStartDeferredIntro(args: {
  scheduledProject: string;
  currentProject: string | null;
  introduced: boolean;
  running: boolean;
  itemCount: number;
  mediaSourceCount: number;
  mediaProxyCount: number;
}): boolean {
  void args;
  return false;
}
