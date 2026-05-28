import { IdentityRow } from "./IdentityRow";

/**
 * TopChrome — the redesigned two-row application chrome.
 *
 * Row 1 (this task): IdentityRow — brand mark, project pill, mode toggle.
 * Row 2 (Task 9):    WorkspaceRow — stage tabs + workspace status.
 */
export function TopChrome() {
  return (
    <div className="flex flex-col border-b border-[var(--color-border-subtle)] bg-[var(--color-surface-app)]">
      <IdentityRow />
      {/* WorkspaceRow lands in Task 9 */}
    </div>
  );
}
