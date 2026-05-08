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

export function SessionBar() {
  const current = useProjectStore((s) => s.current);
  const replaceItems = useAgentStore((s) => s.replace);
  const running = useAgentStore((s) => s.running);
  const [sessions, setSessions] = useState<ChatSessionSummary[]>([]);
  const [activeLogPath, setActiveLogPath] = useState<string>("");
  const [loading, setLoading] = useState(false);

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
    return active ? `${active.messageCount} messages` : "New chat";
  }, [activeLogPath, loading, sessions]);

  async function selectSession(logPath: string) {
    if (!logPath) return;
    setLoading(true);
    try {
      const history = await invoke<ChatHistory>("load_chat_session", { logPath });
      setActiveLogPath(history.session?.logPath ?? "");
      replaceItems(history.items);
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
    } catch (e) {
      console.warn("start_new_chat_session failed", e);
    } finally {
      setLoading(false);
    }
  }

  return (
    <div className="session-bar">
      <div className="session-bar-header">
        <div>
          <span className="session-kicker">Chat sessions</span>
          <span className="session-count">{activeLabel}</span>
        </div>
        <button type="button" disabled={running || loading} onClick={newChat}>
          New
        </button>
      </div>
      <div className="session-list" role="listbox" aria-label="Chat sessions">
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
    </div>
  );
}
