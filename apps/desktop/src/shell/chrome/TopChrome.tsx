import { IdentityRow } from "./IdentityRow";
import { WorkspaceRow } from "./WorkspaceRow";

/**
 * TopChrome — the redesigned two-row application chrome.
 *
 * Row 1: IdentityRow — brand mark, project pill, mode toggle.
 * Row 2: WorkspaceRow — stage tabs + live preview timecode.
 */
export function TopChrome() {
  return (
    <div className="flex flex-col border-b border-[var(--color-border-subtle)] bg-[var(--color-surface-app)]">
      <IdentityRow />
      <WorkspaceRow />
    </div>
  );
}
