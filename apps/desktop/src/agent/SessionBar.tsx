import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Item } from "../protocol";
import { useProjectStore } from "../app/state";
import { useAgentStore } from "./store";

type ChatSessionSummary = {
  id: string;
  title: string;
  projectRoot: string;
  logPath: string;
  startedAt: string;
  messageCount: number;
};

type ChatHistory = {
  session: ChatSessionSummary | null;
  items: Item[];
};

type Props = {
  focused?: boolean;
  onFocusChat?: () => void;
  onRestoreWorkspace?: () => void;
};

export function SessionBar({
  focused = false,
  onFocusChat,
  onRestoreWorkspace,
}: Props) {
  const current = useProjectStore((s) => s.current);
  const replaceItems = useAgentStore((s) => s.replace);
  const running = useAgentStore((s) => s.running);
  const [sessions, setSessions] = useState<ChatSessionSummary[]>([]);
  const [activeLogPath, setActiveLogPath] = useState<string>("");
  const [loading, setLoading] = useState(false);
  const [historyOpen, setHistoryOpen] = useState(false);

  useEffect(() => {
    if (!current) {
      setSessions([]);
      setActiveLogPath("");
      return;
    }
    let cancelled = false;
    setLoading(true);
    Promise.all([
      invoke<ChatSessionSummary[]>("list_chat_sessions"),
      invoke<ChatHistory>("load_chat_history"),
    ])
      .then(([nextSessions, history]) => {
        if (cancelled) return;
        setSessions(nextSessions);
        setActiveLogPath(history.session?.logPath ?? "");
        replaceItems(history.items);
      })
      .catch((e) => console.warn("chat session load failed", e))
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [current, replaceItems]);

  const activeLabel = useMemo(() => {
    if (loading) return "Loading";
    const active = sessions.find((s) => s.logPath === activeLogPath);
    return active ? active.title : "New chat";
  }, [activeLogPath, loading, sessions]);

  const activeMeta = useMemo(() => {
    const active = sessions.find((s) => s.logPath === activeLogPath);
    if (!active) return "Ready";
    return `${active.messageCount} message${active.messageCount === 1 ? "" : "s"}`;
  }, [activeLogPath, sessions]);

  async function selectSession(logPath: string) {
    if (!logPath) return;
    setLoading(true);
    try {
      // Resume binds the codex bridge to this thread so the next
      // turn continues the conversation. Falls back to a read-only
      // load if resume fails (e.g. a turn is running) so the user at
      // least sees the messages.
      let history: ChatHistory;
      try {
        history = await invoke<ChatHistory>("resume_chat_session", { logPath });
      } catch (resumeErr) {
        console.warn("resume_chat_session failed; falling back to read-only load", resumeErr);
        history = await invoke<ChatHistory>("load_chat_session", { logPath });
      }
      setActiveLogPath(history.session?.logPath ?? "");
      replaceItems(history.items);
      setHistoryOpen(false);
    } catch (e) {
      console.warn("load_chat_session failed", e);
    } finally {
      setLoading(false);
    }
  }

  async function newChat() {
    setLoading(true);
    try {
      const history = await invoke<ChatHistory>("start_new_chat_session");
      setActiveLogPath("");
      replaceItems(history.items);
      setHistoryOpen(false);
    } catch (e) {
      console.warn("start_new_chat_session failed", e);
    } finally {
      setLoading(false);
    }
  }

  return (
    <div className="session-bar">
      <div className="session-bar-header">
        <div className="session-current">
          <span className="session-kicker">Current</span>
          <span className="session-count">{activeLabel}</span>
          <span className="session-subtitle">{activeMeta}</span>
        </div>
        <div className="session-actions">
          {(onFocusChat || onRestoreWorkspace) && (
            <button
              type="button"
              className="session-icon-button"
              onClick={focused ? onRestoreWorkspace : onFocusChat}
              title={focused ? "Restore workspace" : "Focus chat"}
              aria-label={focused ? "Restore workspace" : "Focus chat"}
            >
              {focused ? <WorkspaceIcon /> : <FocusIcon />}
            </button>
          )}
          <button
            type="button"
            className="session-icon-button"
            disabled={loading}
            onClick={() => setHistoryOpen((open) => !open)}
            aria-expanded={historyOpen}
            title="Chat history"
            aria-label="Chat history"
          >
            <HistoryIcon />
          </button>
          <button
            type="button"
            className="session-icon-button"
            disabled={running || loading}
            onClick={newChat}
            title="New chat"
            aria-label="New chat"
          >
            <PlusIcon />
          </button>
        </div>
      </div>
      {historyOpen && (
        <div className="session-popover" role="listbox" aria-label="Chat history">
          <div className="session-popover-title">
            <span>Chat history</span>
            <span>{sessions.length} saved</span>
          </div>
          {activeLogPath === "" && (
            <button
              type="button"
              className="session-row session-row-active"
              disabled={running || loading}
              role="option"
              aria-selected="true"
            >
              <span className="session-title">New chat</span>
              <span className="session-meta">Ready</span>
            </button>
          )}
          {sessions.map((session) => {
            const active = session.logPath === activeLogPath;
            return (
              <button
                key={session.logPath}
                type="button"
                className={`session-row ${active ? "session-row-active" : ""}`}
                disabled={running || loading}
                role="option"
                aria-selected={active}
                onClick={() => selectSession(session.logPath)}
              >
                <span className="session-title">{session.title}</span>
                <span className="session-meta">
                  {session.messageCount} message{session.messageCount === 1 ? "" : "s"}
                </span>
              </button>
            );
          })}
          {!loading && sessions.length === 0 && activeLogPath !== "" && (
            <p className="session-empty">No saved chats for this project yet.</p>
          )}
        </div>
      )}
    </div>
  );
}

function FocusIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 16 16">
      <path d="M3 6V3h3M10 3h3v3M13 10v3h-3M6 13H3v-3" />
      <path d="M6.5 8h3" />
    </svg>
  );
}

function WorkspaceIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 16 16">
      <path d="M2.5 4.5h11v7h-11z" />
      <path d="M6.5 13h3M8 11.5V13" />
    </svg>
  );
}

function HistoryIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 16 16">
      <path d="M3 8a5 5 0 1 0 1.5-3.56" />
      <path d="M3 3.5v2.7h2.7M8 5.2V8l2 1.2" />
    </svg>
  );
}

function PlusIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 16 16">
      <path d="M8 3.5v9M3.5 8h9" />
    </svg>
  );
}
