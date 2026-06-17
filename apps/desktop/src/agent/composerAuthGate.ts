import type { AuthStatus } from "../state/auth";

type ComposerAuthGateInput = {
  projectReady: boolean;
  running: boolean;
  authStatus: AuthStatus | null;
  text?: string;
};

export type ComposerAuthGateState = {
  authReady: boolean;
  disabledReason: string | null;
  textareaDisabled: boolean;
  sendDisabled: boolean;
  sendLabel: string;
  placeholder: string;
};

export function isAuthReadyForAgent(status: AuthStatus | null): boolean {
  return Boolean(status && status.mode !== "none");
}

export function composerAuthGateState({
  projectReady,
  running,
  authStatus,
  text = "",
}: ComposerAuthGateInput): ComposerAuthGateState {
  const authReady = isAuthReadyForAgent(authStatus);
  const disabledReason = authReady ? null : "Sign in to get started";
  const hasText = text.trim().length > 0;
  return {
    authReady,
    disabledReason,
    textareaDisabled: running || !projectReady || !authReady,
    sendDisabled: !projectReady || (!disabledReason && !hasText),
    sendLabel: disabledReason ?? "Send",
    placeholder: projectReady
      ? disabledReason ?? "Message Montage"
      : "Open a project to start chatting",
  };
}
