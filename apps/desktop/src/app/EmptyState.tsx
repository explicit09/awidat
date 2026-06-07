// Shown in the chat pane when the project has no items yet.
// Points the user at the persistent ActionBar above; doesn't
// duplicate buttons (those live in ActionBar permanently).

export function EmptyState() {
  return (
    <div className="empty-state">
      <span className="empty-kicker">Ready for media</span>
      <h2>Build the cut from source, transcript, or agent proposal.</h2>
      <div className="empty-steps" aria-label="Startup workflow">
        <span>Import</span>
        <span>Index</span>
        <span>Ask</span>
        <span>Review</span>
        <span>Export</span>
      </div>
      <p className="empty-hint">
        Use the command bar to import local files or a URL. Montage will create
        proxies, run indexers, and stream the job state into the copilot rail.
      </p>
    </div>
  );
}
