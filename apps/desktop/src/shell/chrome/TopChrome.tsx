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
    <div className="glass-strong flex flex-col border-b border-[var(--glass-border)]" style={{ borderRadius: 0 }}>
      <IdentityRow />
      <WorkspaceRow />
    </div>
  );
}
