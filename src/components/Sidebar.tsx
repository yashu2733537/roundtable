import { useState } from "react";
import type { RefObject } from "react";
import EmptyState from "./EmptyState";
import { IconAgents, IconCommand, IconEdit, IconRooms, IconSliders, IconTrash } from "./Icon";
import { errorMessage } from "../lib/tauri";
import type { Conversation } from "../lib/api";

interface SidebarProps {
  paneRef: RefObject<HTMLElement>;
  conversations: Conversation[];
  activeId: string | null;
  loading: boolean;
  loadError: string | null;
  onReload: () => void;
  onNew: () => void;
  onSelect: (id: string) => void;
  onRename: (id: string, title: string) => Promise<void>;
  onDelete: (id: string) => Promise<void>;
  onSearch: (query: string) => void;
  searchQuery: string;
  onOpenPalette: () => void;
  onOpenSettings: () => void;
  onShowAgents: () => void;
  onShowRooms: () => void;
}

export default function Sidebar({
  paneRef,
  conversations,
  activeId,
  loading,
  loadError,
  onReload,
  onNew,
  onSelect,
  onRename,
  onDelete,
  onSearch,
  searchQuery,
  onOpenPalette,
  onOpenSettings,
  onShowAgents,
  onShowRooms,
}: SidebarProps) {
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameText, setRenameText] = useState("");
  const [pendingDeleteId, setPendingDeleteId] = useState<string | null>(null);
  const [rowError, setRowError] = useState<string | null>(null);

  const startRename = (conv: Conversation) => {
    setRowError(null);
    setPendingDeleteId(null);
    setRenamingId(conv.id);
    setRenameText(conv.title);
  };

  const commitRename = async (id: string) => {
    setRowError(null);
    try {
      await onRename(id, renameText);
      setRenamingId(null);
    } catch (err: unknown) {
      setRowError(errorMessage(err));
    }
  };

  const askDelete = (id: string) => {
    setRowError(null);
    setRenamingId(null);
    setPendingDeleteId(id);
    window.setTimeout(() => {
      setPendingDeleteId((current) => (current === id ? null : current));
    }, 4000);
  };

  const commitDelete = async (id: string) => {
    setRowError(null);
    try {
      await onDelete(id);
      setPendingDeleteId(null);
    } catch (err: unknown) {
      setRowError(errorMessage(err));
    }
  };

  return (
    <aside
      className="pane pane--sidebar"
      ref={paneRef}
      tabIndex={-1}
      aria-label="Conversations"
    >
      <header className="pane__header">
        <span className="brand">
          <span className="brand__name">Roundtable</span>
        </span>
        <div className="pane__header-actions">
          <button
            type="button"
            className="btn btn--icon"
            onClick={onShowAgents}
            aria-label="Open agents"
            title="Agents"
          >
            <IconAgents />
          </button>
          <button
            type="button"
            className="btn btn--icon"
            onClick={onShowRooms}
            aria-label="Open rooms"
            title="Rooms"
          >
            <IconRooms />
          </button>
          <button
            type="button"
            className="btn btn--icon"
            onClick={onOpenPalette}
            aria-label="Open command palette"
            title="Command palette (Ctrl+K)"
          >
            <IconCommand />
          </button>
          <button
            type="button"
            className="btn btn--icon"
            onClick={onOpenSettings}
            aria-label="Open settings"
            title="Settings"
          >
            <IconSliders />
          </button>
        </div>
      </header>
      <div className="sidebar__tools">
        <button type="button" className="btn btn--primary btn--block" onClick={onNew}>
          New chat
        </button>
        <input
          type="search"
          className="input"
          placeholder="Search chats…"
          aria-label="Search conversations"
          value={searchQuery}
          onChange={(e) => onSearch(e.target.value)}
        />
      </div>
      <div className="pane__body sidebar__list" role="listbox" aria-label="Conversation list">
        {loading && (
          <p className="muted" role="status">
            <span className="spinner" aria-hidden="true" /> Loading chats…
          </p>
        )}
        {!loading && loadError && (
          <div className="notice notice--error" role="alert">
            <p className="notice__title">Couldn’t load chats.</p>
            <p className="notice__body">{loadError}</p>
            <p className="notice__body">
              <button type="button" className="btn" onClick={onReload}>
                Retry
              </button>
            </p>
          </div>
        )}
        {!loading && !loadError && conversations.length === 0 && (
          <EmptyState
            title={searchQuery ? "No matches" : "No conversations yet"}
            description={
              searchQuery
                ? "Nothing matches that search."
                : "Start a new chat to talk to a model."
            }
          />
        )}
        {!loading &&
          !loadError &&
          conversations.map((conv) => (
            <div
              key={conv.id}
              role="option"
              aria-selected={conv.id === activeId}
              tabIndex={0}
              className={`chat-row${conv.id === activeId ? " chat-row--active" : ""}`}
              onClick={() => onSelect(conv.id)}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  onSelect(conv.id);
                }
              }}
            >
              {renamingId === conv.id ? (
                <form
                  className="chat-row__rename"
                  onSubmit={(e) => {
                    e.preventDefault();
                    void commitRename(conv.id);
                  }}
                  onClick={(e) => e.stopPropagation()}
                >
                  <input
                    className="input"
                    aria-label="Conversation title"
                    value={renameText}
                    onChange={(e) => setRenameText(e.target.value)}
                    autoFocus
                  />
                  <button type="submit" className="btn">
                    Save
                  </button>
                  <button
                    type="button"
                    className="btn"
                    onClick={() => setRenamingId(null)}
                  >
                    Cancel
                  </button>
                </form>
              ) : (
                <>
                  <span className="chat-row__title">{conv.title}</span>
                  <span className="chat-row__actions" onClick={(e) => e.stopPropagation()}>
                    <button
                      type="button"
                      className="btn btn--icon btn--sm"
                      aria-label={`Rename ${conv.title}`}
                      title="Rename"
                      onClick={() => startRename(conv)}
                    >
                      <IconEdit />
                    </button>
                    {pendingDeleteId === conv.id ? (
                      <button
                        type="button"
                        className="btn btn--danger btn--sm"
                        onClick={() => void commitDelete(conv.id)}
                      >
                        Confirm?
                      </button>
                    ) : (
                      <button
                        type="button"
                        className="btn btn--icon btn--sm"
                        aria-label={`Delete ${conv.title}`}
                        title="Delete"
                        onClick={() => askDelete(conv.id)}
                      >
                        <IconTrash />
                      </button>
                    )}
                  </span>
                </>
              )}
            </div>
          ))}
        {rowError && (
          <div className="notice notice--error" role="alert">
            <p className="notice__body">{rowError}</p>
          </div>
        )}
      </div>
    </aside>
  );
}
