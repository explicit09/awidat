// Shows the currently-loaded project root and lets the user change
// it. Step 1 has no proper file picker — users paste a path into a
// text input. Step 2 will replace this with a directory picker.

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type Props = {
  /** Called whenever the project root successfully changes. */
  onChange: (path: string | null) => void;
};

export function ProjectBanner({ onChange }: Props) {
  const [current, setCurrent] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<string | null>("current_project_root")
      .then((p) => {
        setCurrent(p);
        onChange(p);
      })
      .catch((err) => setError(String(err)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function save() {
    const p = draft.trim();
    if (!p) return;
    setError(null);
    try {
      await invoke("set_project_root", { path: p });
      setCurrent(p);
      setEditing(false);
      setDraft("");
      onChange(p);
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <div className="project-banner">
      <span className="project-label">project:</span>
      {editing ? (
        <form
          className="project-form"
          onSubmit={(e) => {
            e.preventDefault();
            save();
          }}
        >
          <input
            type="text"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            placeholder="/absolute/path/to/project"
            autoFocus
          />
          <button type="submit">Set</button>
          <button type="button" onClick={() => setEditing(false)}>
            Cancel
          </button>
        </form>
      ) : (
        <>
          <code className="project-path">{current ?? "(none — set one to start)"}</code>
          <button
            onClick={() => {
              setDraft(current ?? "");
              setEditing(true);
            }}
          >
            {current ? "Change" : "Set"}
          </button>
        </>
      )}
      {error && <span className="project-error">{error}</span>}
    </div>
  );
}
