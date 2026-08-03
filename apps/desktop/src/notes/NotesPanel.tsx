import { useMediaStore } from "../media/store";
import type { EditorialNoteStatus } from "../protocol";
import { type Note, useNotesStore } from "./store";

export function NotesPanel() {
  const notes = useNotesStore((state) => state.notes);
  const busy = useNotesStore((state) => state.busy);
  const setStatus = useNotesStore((state) => state.setStatus);
  const openCount = notes.filter((note) => note.status === "open").length;
  const resolvedCount = notes.filter((note) => note.status === "resolved").length;
  const dismissedCount = notes.filter((note) => note.status === "dismissed").length;

  return (
    <section className="flex h-full min-h-0 flex-col">
      <header className="border-b border-[var(--glass-border)] px-4 py-3">
        <h2 className="text-[14px] font-semibold text-[var(--color-text-primary)]">
          Editorial Notes
        </h2>
        <div className="mt-1 flex gap-3 text-[11px] text-[var(--color-text-muted)]">
          <span>{openCount} open</span>
          <span>{resolvedCount} resolved</span>
          <span>{dismissedCount} dismissed</span>
        </div>
      </header>
      <div className="min-h-0 flex-1 space-y-2 overflow-y-auto p-3">
        {notes.length === 0 ? (
          <p className="px-1 py-4 text-[12px] text-[var(--color-text-muted)]">
            No editorial notes yet.
          </p>
        ) : (
          notes.map((note) => (
            <NoteCard
              key={note.id}
              note={note}
              busy={busy}
              onStatus={setStatus}
            />
          ))
        )}
      </div>
    </section>
  );
}

function NoteCard({
  note,
  busy,
  onStatus,
}: {
  note: Note;
  busy: boolean;
  onStatus: (id: string, status: EditorialNoteStatus) => Promise<void>;
}) {
  const time = formatAnchorTime(note.anchorAtS);
  const goToAnchor = () => {
    useMediaStore.getState().requestTimelineSeek(note.anchorAtS);
  };

  return (
    <article className="rounded-lg border border-[var(--glass-border)] bg-black/20 p-3">
      <div className="flex items-start gap-2">
        <span className="rounded bg-white/5 px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-[var(--color-text-muted)]">
          {note.kind.replace(/_/g, " ")}
        </span>
        <span className="ml-auto text-[10px] uppercase tracking-wide text-[var(--color-text-muted)]">
          {note.status}
        </span>
      </div>
      <p className="mt-2 text-[13px] leading-5 text-[var(--color-text-primary)]">
        {note.summary}
      </p>
      {note.continuityReasons?.length ? (
        <ul className="mt-2 list-disc space-y-1 pl-4 text-[11px] text-[var(--color-text-secondary)]">
          {note.continuityReasons.map((reason) => <li key={reason}>{reason}</li>)}
        </ul>
      ) : null}
      {note.brollQuery ? (
        <p className="mt-2 text-[11px] text-[var(--color-text-muted)]">
          Search: {note.brollQuery}
        </p>
      ) : null}
      <div className="mt-3 flex flex-wrap items-center gap-2">
        <button
          type="button"
          aria-label={`Go to ${time}`}
          onClick={goToAnchor}
          className="glass-ghost rounded px-2 py-1 text-[11px] text-[var(--color-text-secondary)]"
        >
          {time}
        </button>
        {note.status === "open" ? (
          <>
            <button
              type="button"
              disabled={busy}
              onClick={() => void onStatus(note.id, "resolved")}
              className="glass-ghost ml-auto rounded px-2 py-1 text-[11px] disabled:opacity-50"
            >
              Resolve
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={() => void onStatus(note.id, "dismissed")}
              className="glass-ghost rounded px-2 py-1 text-[11px] disabled:opacity-50"
            >
              Dismiss
            </button>
          </>
        ) : (
          <button
            type="button"
            disabled={busy}
            onClick={() => void onStatus(note.id, "open")}
            className="glass-ghost ml-auto rounded px-2 py-1 text-[11px] disabled:opacity-50"
          >
            Reopen
          </button>
        )}
      </div>
    </article>
  );
}

function formatAnchorTime(seconds: number): string {
  const centiseconds = Math.max(0, Math.round(seconds * 100));
  const minutes = Math.floor(centiseconds / 6000);
  const remainder = centiseconds % 6000;
  const wholeSeconds = Math.floor(remainder / 100);
  const fraction = remainder % 100;
  return `${String(minutes).padStart(2, "0")}:${String(wholeSeconds).padStart(2, "0")}.${String(fraction).padStart(2, "0")}`;
}
