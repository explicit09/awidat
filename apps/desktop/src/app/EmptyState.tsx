// Shown in the chat pane when the project has no items yet.
// Points the user at the persistent ActionBar above; doesn't
// duplicate buttons (those live in ActionBar permanently).

export function EmptyState() {
  return (
    <div className="empty-state">
      <h2>Drop media in to get started</h2>
      <p className="empty-hint">
        Use the bar above to <strong>Import file…</strong>,{" "}
        <strong>Import URL…</strong>, or <strong>Run indexers</strong> on
        media you've already dropped into <code>raw/</code>. Imports
        auto-chain through proxy transcoding and indexing — three job
        cards stream in here as the work progresses.
      </p>
      <p className="empty-hint">
        Once indexing is done, ask the agent anything: "what's the
        funniest line?", "cut the first 30 seconds", "find the call
        to action".
      </p>
    </div>
  );
}
