import { useEffect, useMemo, useRef, useState } from "react";
import type { RefObject } from "react";
import EmptyState from "./EmptyState";
import ModelPicker, { type ModelValue } from "./ModelPicker";
import { AgentAvatar } from "./AgentsPage";
import { IconCommand, IconCopy, IconEdit, IconPanelDetail, IconStop } from "./Icon";
import { errorMessage } from "../lib/tauri";
import { spacerHeights, useVirtualWindow } from "./useVirtualList";
import type {
  Agent,
  ChatMessage,
  Conversation,
  OllamaStatus,
  ProviderInfo,
} from "../lib/api";

interface ConversationPaneProps {
  paneRef: RefObject<HTMLElement>;
  conversation: Conversation | null;
  messages: ChatMessage[];
  messagesLoading: boolean;
  messagesError: string | null;
  streaming: boolean;
  draft: string;
  lastStopped: boolean;
  streamError: string | null;
  composer: string;
  modelValue: ModelValue;
  providers: ProviderInfo[];
  ollama: OllamaStatus;
  modelsByProvider: Record<string, string[]>;
  modelsLoading: boolean;
  agents: Agent[];
  agentId: string | null;
  onAgentChange: (agentId: string | null) => void;
  onComposerChange: (text: string) => void;
  onSend: (overrideAgentId: string | null) => void;
  onStop: () => void;
  onRegenerate: (overrideAgentId: string | null) => void;
  onEditResend: (messageId: string, content: string, overrideAgentId: string | null) => void;
  onModelChange: (value: ModelValue) => void;
  onRefreshModels: (providerId: string) => void;
  onNewChat: () => void;
  onOpenPalette: () => void;
  onOpenSettings: () => void;
  onShowAgents: () => void;
  onOpenInspector?: () => void;
}

interface MentionState {
  query: string;
  start: number;
}

export default function ConversationPane({
  paneRef,
  conversation,
  messages,
  messagesLoading,
  messagesError,
  streaming,
  draft,
  lastStopped,
  streamError,
  composer,
  modelValue,
  providers,
  ollama,
  modelsByProvider,
  modelsLoading,
  agents,
  agentId,
  onAgentChange,
  onComposerChange,
  onSend,
  onStop,
  onRegenerate,
  onEditResend,
  onModelChange,
  onRefreshModels,
  onNewChat,
  onOpenPalette,
  onOpenSettings,
  onShowAgents,
  onOpenInspector,
}: ConversationPaneProps) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const stickRef = useRef(true);
  const composerRef = useRef<HTMLTextAreaElement>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editText, setEditText] = useState("");
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const [mention, setMention] = useState<MentionState | null>(null);
  const [mentionIndex, setMentionIndex] = useState(0);

  // Virtualized message window: full list under the threshold, slice + spacers above it.
  const { start: vStart, end: vEnd, virtualized, onScrollMeas } = useVirtualWindow(
    messages.length,
    scrollRef,
    { estimatedHeight: 132, overscan: 12, threshold: 80 },
  );
  const visibleMessages = virtualized ? messages.slice(vStart, vEnd) : messages;
  const { top: vTopPx, bottom: vBottomPx } = spacerHeights(
    messages.length,
    vStart,
    vEnd,
    132,
  );

  const handleScroll = (e: React.UIEvent<HTMLDivElement>) => {
    const el = e.currentTarget;
    stickRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
    onScrollMeas();
  };

  // Stick to the bottom while new tokens arrive, unless the user scrolled up.
  useEffect(() => {
    const el = scrollRef.current;
    if (el && stickRef.current) {
      el.scrollTop = el.scrollHeight;
    }
  }, [messages, draft, streaming]);

  useEffect(() => {
    setEditingId(null);
    setMention(null);
  }, [conversation?.id]);

  const enabledAgents = useMemo(() => agents.filter((a) => a.enabled), [agents]);
  const boundAgent = agents.find((a) => a.id === agentId) ?? null;

  const mentionMatches = useMemo(() => {
    if (!mention) return [];
    const query = mention.query.toLowerCase();
    return enabledAgents.filter((a) => a.name.toLowerCase().includes(query)).slice(0, 6);
  }, [mention, enabledAgents]);

  useEffect(() => {
    setMentionIndex(0);
  }, [mention?.query]);

  const detectMention = (text: string, cursor: number) => {
    const before = text.slice(0, cursor);
    const match = /@([\w-]*)$/.exec(before);
    if (!match) {
      setMention(null);
      return;
    }
    setMention({ query: match[1], start: cursor - match[0].length });
  };

  const acceptMention = (agent: Agent) => {
    if (!mention) return;
    const cursor = composerRef.current?.selectionStart ?? composer.length;
    const before = composer.slice(0, mention.start);
    const after = composer.slice(cursor);
    const next = `${before}@${agent.name} ${after}`;
    onComposerChange(next);
    setMention(null);
    requestAnimationFrame(() => {
      const el = composerRef.current;
      if (!el) return;
      const pos = before.length + agent.name.length + 2;
      el.focus();
      el.setSelectionRange(pos, pos);
    });
  };

  /** First @Name in the composer that matches an enabled agent, if any. */
  const mentionOverride = (): string | null => {
    const pattern = /@([\w-]+)/g;
    let match: RegExpExecArray | null;
    while ((match = pattern.exec(composer)) !== null) {
      const name = match[1].toLowerCase();
      const agent = enabledAgents.find((a) => a.name.toLowerCase() === name);
      if (agent) return agent.id;
    }
    return null;
  };

  const lastMessage = messages[messages.length - 1] ?? null;
  const canRegenerate =
    !streaming && lastMessage?.role === "assistant" && conversation != null;
  const lastUserMessage =
    [...messages].reverse().find((m) => m.role === "user") ?? null;
  const editableUserId = !streaming ? (lastUserMessage?.id ?? null) : null;

  const agentForMessage = (message: ChatMessage): Agent | null => {
    if (!message.agent_id) return null;
    return agents.find((a) => a.id === message.agent_id) ?? null;
  };

  const copyMessage = async (id: string, content: string) => {
    try {
      await navigator.clipboard.writeText(content);
    } catch {
      try {
        const area = document.createElement("textarea");
        area.value = content;
        document.body.appendChild(area);
        area.select();
        document.execCommand("copy");
        document.body.removeChild(area);
      } catch (err: unknown) {
        void errorMessage(err);
        return;
      }
    }
    setCopiedId(id);
    window.setTimeout(() => {
      setCopiedId((current) => (current === id ? null : current));
    }, 1500);
  };

  const startEdit = (message: ChatMessage) => {
    setEditText(message.content);
    setEditingId(message.id);
  };

  const sendWithMention = () => {
    onSend(mentionOverride());
    setMention(null);
  };

  return (
    <main
      className="pane pane--conversation"
      ref={paneRef}
      tabIndex={-1}
      aria-label="Conversation"
    >
      <header className="pane__header">
        <h1 className="pane__title">
          {conversation ? conversation.title : "Conversation"}
        </h1>
        <div className="pane__header-actions">
          {conversation && onOpenInspector && (
            <button
              type="button"
              className="btn btn--icon"
              onClick={onOpenInspector}
              aria-label="Open inspector"
              title="Inspector"
            >
              <IconPanelDetail />
            </button>
          )}
          {conversation && (
            <select
              className="select agent-switcher"
              aria-label="Chat agent"
              title="Answering agent for this conversation"
              value={agentId ?? ""}
              disabled={streaming}
              onChange={(e) => onAgentChange(e.target.value === "" ? null : e.target.value)}
            >
              <option value="">No agent</option>
              {agents
                .filter((a) => a.enabled || a.id === agentId)
                .map((a) => (
                  <option key={a.id} value={a.id}>
                    {a.enabled ? a.name : `${a.name} (disabled)`}
                  </option>
                ))}
            </select>
          )}
          <button
            type="button"
            className="btn"
            disabled={!canRegenerate}
            onClick={() => onRegenerate(mentionOverride())}
            title="Regenerate the last assistant reply"
          >
            Regenerate
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
        </div>
      </header>

      {conversation == null ? (
        <div className="pane__body pane__body--center">
          <EmptyState
            mark
            title="No conversation open"
            description="Start a new chat to talk to a model or an agent."
          >
            <p className="empty-state__hint">
              <button type="button" className="btn btn--primary" onClick={onNewChat}>
                New chat
              </button>
            </p>
            <p className="empty-state__hint">
              Press <kbd>Ctrl</kbd> <kbd>K</kbd> for the command palette.
            </p>
          </EmptyState>
        </div>
      ) : (
        <>
          <div
            className="pane__body chat__scroll"
            ref={scrollRef}
            onScroll={handleScroll}
          >
            {providers.length === 0 && agentId == null && (
              <div className="notice notice--error" role="alert">
                <p className="notice__title">No providers yet.</p>
                <p className="notice__body">
                  Add a provider in Settings to start chatting.{" "}
                  <button type="button" className="btn" onClick={onOpenSettings}>
                    Open settings
                  </button>
                </p>
              </div>
            )}
            {boundAgent && !boundAgent.enabled && (
              <div className="notice" role="note">
                <p className="notice__body">
                  {boundAgent.name} is disabled, so this chat falls back to its
                  own provider and model.{" "}
                  <button type="button" className="btn" onClick={onShowAgents}>
                    Manage agents
                  </button>
                </p>
              </div>
            )}
            {messagesLoading && (
              <p className="muted" role="status">
                <span className="spinner" aria-hidden="true" /> Loading messages…
              </p>
            )}
            {messagesError && (
              <div className="notice notice--error" role="alert">
                <p className="notice__title">Couldn’t load messages.</p>
                <p className="notice__body">{messagesError}</p>
              </div>
            )}
            {!messagesLoading && !messagesError && messages.length === 0 && !streaming && (
              <EmptyState
                title="Say hello"
                description="Pick a model or agent below, then write your first message. Type @ to route a message to a specific agent."
              />
            )}
            <ol className="chat__list">
              {virtualized && vTopPx > 0 && (
                <div className="virt-spacer" style={{ height: vTopPx }} aria-hidden="true" />
              )}
              {visibleMessages.map((message) => {
                const agent = message.role === "assistant" ? agentForMessage(message) : null;
                return (
                  <li key={message.id} className={`msg msg--${message.role}`}>
                    <div className="msg__head">
                      <span className="msg__role">
                        {message.role === "user" ? (
                          "You"
                        ) : agent ? (
                          <>
                            <AgentAvatar agent={agent} size={18} /> {agent.name}
                          </>
                        ) : (
                          "Assistant"
                        )}
                      </span>
                      <span className="msg__actions">
                        <button
                          type="button"
                          className="btn btn--icon btn--sm"
                          aria-label="Copy message"
                          title={copiedId === message.id ? "Copied!" : "Copy"}
                          onClick={() => void copyMessage(message.id, message.content)}
                        >
                          <IconCopy />
                        </button>
                        {message.role === "user" && message.id === editableUserId && (
                          <button
                            type="button"
                            className="btn btn--icon btn--sm"
                            aria-label="Edit and resend"
                            title="Edit and resend"
                            onClick={() => startEdit(message)}
                          >
                            <IconEdit />
                          </button>
                        )}
                      </span>
                    </div>
                    {editingId === message.id ? (
                      <form
                        className="msg__edit"
                        onSubmit={(e) => {
                          e.preventDefault();
                          onEditResend(message.id, editText, mentionOverride());
                          setEditingId(null);
                        }}
                      >
                        <textarea
                          className="textarea"
                          aria-label="Edit message"
                          value={editText}
                          onChange={(e) => setEditText(e.target.value)}
                          rows={4}
                          autoFocus
                        />
                        <div className="msg__edit-actions">
                          <button type="submit" className="btn btn--primary">
                            Send
                          </button>
                          <button
                            type="button"
                            className="btn"
                            onClick={() => setEditingId(null)}
                          >
                            Cancel
                          </button>
                        </div>
                      </form>
                    ) : (
                      <p className="msg__text">{message.content}</p>
                    )}
                  </li>
                );
              })}
              {streaming && (
                <li className="msg msg--assistant" aria-live="polite">
                  <div className="msg__head">
                    <span className="msg__role">Assistant</span>
                  </div>
                  <p className="msg__text">
                    {draft}
                    <span className="chat__cursor" aria-hidden="true" />
                  </p>
                </li>
              )}
              {virtualized && vBottomPx > 0 && (
                <div className="virt-spacer" style={{ height: vBottomPx }} aria-hidden="true" />
              )}
            </ol>
            {lastStopped && !streaming && (
              <p className="muted" role="status">
                Stopped — the partial reply above is saved.
              </p>
            )}
            {streamError && (
              <div className="notice notice--error" role="alert">
                <p className="notice__title">The reply failed.</p>
                <p className="notice__body">{streamError}</p>
              </div>
            )}
          </div>

          <div className="chat__dock">
            {boundAgent ? (
              <p className="muted" role="note">
                Answering as {boundAgent.name} ({boundAgent.model || "no model set"}).
                Change the model in the agent, or unbind it above.
              </p>
            ) : (
              <ModelPicker
                providers={providers}
                ollama={ollama}
                modelsByProvider={modelsByProvider}
                modelsLoading={modelsLoading}
                value={modelValue}
                disabled={streaming}
                onChange={onModelChange}
                onRefreshModels={onRefreshModels}
              />
            )}
            <form
              className="chat__composer"
              onSubmit={(e) => {
                e.preventDefault();
                if (mention && mentionMatches.length > 0) {
                  acceptMention(mentionMatches[mentionIndex] ?? mentionMatches[0]);
                  return;
                }
                if (!streaming) sendWithMention();
              }}
            >
              <div className="chat__input-wrap">
                <textarea
                  ref={composerRef}
                  className="textarea chat__input"
                  aria-label="Message"
                  placeholder="Write a message… (Enter sends, Shift+Enter adds a line, @ mentions an agent)"
                  value={composer}
                  onChange={(e) => {
                    onComposerChange(e.target.value);
                    detectMention(
                      e.target.value,
                      e.target.selectionStart ?? e.target.value.length,
                    );
                  }}
                  onClick={(e) =>
                    detectMention(
                      composer,
                      e.currentTarget.selectionStart ?? composer.length,
                    )
                  }
                  onKeyUp={(e) => {
                    if (e.key.length === 1 || e.key === "Backspace") {
                      const el = e.currentTarget;
                      detectMention(composer, el.selectionStart ?? composer.length);
                    }
                  }}
                  onKeyDown={(e) => {
                    if (mention && mentionMatches.length > 0) {
                      if (e.key === "ArrowDown") {
                        e.preventDefault();
                        setMentionIndex((i) => (i + 1) % mentionMatches.length);
                        return;
                      }
                      if (e.key === "ArrowUp") {
                        e.preventDefault();
                        setMentionIndex(
                          (i) => (i - 1 + mentionMatches.length) % mentionMatches.length,
                        );
                        return;
                      }
                      if (e.key === "Escape") {
                        e.preventDefault();
                        setMention(null);
                        return;
                      }
                      if (e.key === "Tab" || e.key === "Enter") {
                        e.preventDefault();
                        acceptMention(mentionMatches[mentionIndex] ?? mentionMatches[0]);
                        return;
                      }
                    } else if (e.key === "Escape") {
                      setMention(null);
                    }
                    if (e.key === "Enter" && !e.shiftKey) {
                      e.preventDefault();
                      if (!streaming) sendWithMention();
                    }
                  }}
                  rows={3}
                  disabled={streaming}
                />
                {mention && mentionMatches.length > 0 && (
                  <ul className="mention-list" role="listbox" aria-label="Mention an agent">
                    {mentionMatches.map((agent, index) => (
                      <li key={agent.id} role="option" aria-selected={index === mentionIndex}>
                        <button
                          type="button"
                          className={`mention-item${index === mentionIndex ? " mention-item--active" : ""}`}
                          onMouseDown={(e) => {
                            // Accept before the textarea loses focus.
                            e.preventDefault();
                            acceptMention(agent);
                          }}
                          onMouseEnter={() => setMentionIndex(index)}
                        >
                          <AgentAvatar agent={agent} size={20} /> {agent.name}
                        </button>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
              {streaming ? (
                <button
                  type="button"
                  className="btn btn--danger chat__send"
                  onClick={onStop}
                  aria-label="Stop generating"
                >
                  <IconStop /> Stop
                </button>
              ) : (
                <button
                  type="submit"
                  className="btn btn--primary chat__send"
                  disabled={composer.trim() === ""}
                >
                  Send
                </button>
              )}
            </form>
          </div>
        </>
      )}
    </main>
  );
}
