import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import Sidebar from "./components/Sidebar";
import ConversationPane from "./components/ConversationPane";
import AgentsPage from "./components/AgentsPage";
import RoomsView from "./components/RoomsView";
import Inspector, { type RunInfoState } from "./components/Inspector";
import BottomNav, { type MobileNav, type MobilePane } from "./components/BottomNav";
import { IconClose } from "./components/Icon";
import CommandPalette, { type Command } from "./components/CommandPalette";
import SettingsDialog from "./components/SettingsDialog";
import { useTheme, type ThemeChoice } from "./lib/theme";
import { errorMessage, fetchAppInfo } from "./lib/tauri";
import {
  cancelChat,
  createConversation,
  deleteConversation,
  editResendChat,
  fetchAgentTemplates,
  fetchOllamaStatus,
  fetchRunInfo,
  getConversation,
  getSettings,
  listConversations,
  listMessages,
  listProviders,
  newStreamId,
  providerModels,
  regenerateChat,
  renameConversation,
  searchAgents,
  searchConversations,
  sendChat,
  setConversationAgent,
  setConversationModel,
  setSettings,
  type Agent,
  type AgentTemplate,
  type ChatMessage,
  type Conversation,
  type OllamaStatus,
  type ProviderInfo,
  type StreamEvent,
} from "./lib/api";
import type { ModelValue } from "./components/ModelPicker";
import type { AppInfoState } from "./lib/types";

const NARROW_QUERY = "(max-width: 899px)";

type CenterView = "chat" | "agents" | "rooms";

export default function App() {
  const { choice: theme, setChoice: setTheme } = useTheme();
  const [isNarrow, setIsNarrow] = useState(
    () => window.matchMedia(NARROW_QUERY).matches,
  );
  const [mobilePane, setMobilePane] = useState<MobilePane>("conversation");
  const [centerView, setCenterView] = useState<CenterView>("chat");
  const [inspectorOpen, setInspectorOpen] = useState(false);
  const inspectorOpenerRef = useRef<HTMLElement | null>(null);
  const sheetRef = useRef<HTMLDivElement>(null);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [appInfo, setAppInfo] = useState<AppInfoState>({ status: "loading" });

  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [chatsLoading, setChatsLoading] = useState(true);
  const [chatsError, setChatsError] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState("");
  const [activeId, setActiveId] = useState<string | null>(null);

  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [messagesLoading, setMessagesLoading] = useState(false);
  const [messagesError, setMessagesError] = useState<string | null>(null);
  const [composer, setComposer] = useState("");

  const [streamId, setStreamId] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [streamError, setStreamError] = useState<string | null>(null);
  const [lastStopped, setLastStopped] = useState(false);

  const [providers, setProviders] = useState<ProviderInfo[]>([]);
  const [ollama, setOllama] = useState<OllamaStatus>({ running: false, models: [] });
  const [modelsByProvider, setModelsByProvider] = useState<Record<string, string[]>>({});
  const [modelsLoading, setModelsLoading] = useState(false);
  const [defaultProviderId, setDefaultProviderId] = useState<string | null>(null);
  const [defaultModel, setDefaultModel] = useState("");

  const [agents, setAgents] = useState<Agent[]>([]);
  const [agentsLoading, setAgentsLoading] = useState(true);
  const [agentsError, setAgentsError] = useState<string | null>(null);
  const [templates, setTemplates] = useState<AgentTemplate[]>([]);
  const [run, setRun] = useState<RunInfoState>({ status: "idle" });

  const chatsRef = useRef<HTMLElement>(null);
  const conversationRef = useRef<HTMLElement>(null);
  const inspectorRef = useRef<HTMLElement>(null);

  // Track the responsive breakpoint in JS: below it we render a single pane.
  useEffect(() => {
    const mq = window.matchMedia(NARROW_QUERY);
    const onChange = (event: MediaQueryListEvent) => setIsNarrow(event.matches);
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);

  // One-time query to the shared Rust core.
  useEffect(() => {
    let cancelled = false;
    fetchAppInfo()
      .then((info) => {
        if (!cancelled) setAppInfo({ status: "ready", info });
      })
      .catch((err: unknown) => {
        if (!cancelled) setAppInfo({ status: "error", message: errorMessage(err) });
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const refreshConversations = useCallback(async () => {
    try {
      const list = searchQuery.trim()
        ? await searchConversations(searchQuery)
        : await listConversations();
      setConversations(list);
      setChatsError(null);
    } catch (err: unknown) {
      setChatsError(errorMessage(err));
    } finally {
      setChatsLoading(false);
    }
  }, [searchQuery]);

  const refreshAgents = useCallback(async () => {
    try {
      setAgents(await searchAgents("", undefined));
      setAgentsError(null);
    } catch (err: unknown) {
      setAgentsError(errorMessage(err));
    } finally {
      setAgentsLoading(false);
    }
  }, []);

  const refreshRun = useCallback(async (conversationId: string | null) => {
    if (conversationId == null) {
      setRun({ status: "idle" });
      return;
    }
    setRun({ status: "loading" });
    try {
      setRun({ status: "ready", run: await fetchRunInfo(conversationId) });
    } catch (err: unknown) {
      setRun({ status: "error", message: errorMessage(err) });
    }
  }, []);

  // Initial load: chats, agents, templates, providers, model lists, defaults.
  useEffect(() => {
    void refreshConversations();
    void refreshAgents();
    fetchAgentTemplates()
      .then(setTemplates)
      .catch(() => undefined);
    listProviders()
      .then((rows) => {
        setProviders(rows);
        void Promise.allSettled(
          rows.map((p) =>
            providerModels(p.id)
              .then((models) =>
                setModelsByProvider((prev) => ({ ...prev, [p.id]: models })),
              )
              .catch(() => undefined),
          ),
        );
      })
      .catch(() => undefined);
    getSettings()
      .then((settings) => {
        setDefaultProviderId(settings["default.provider"] ?? null);
        setDefaultModel(settings["default.model"] ?? "");
      })
      .catch(() => undefined);
    fetchOllamaStatus()
      .then(setOllama)
      .catch(() => undefined);
  }, [refreshConversations, refreshAgents]);

  // Debounced sidebar search.
  useEffect(() => {
    setChatsLoading(true);
    const timer = window.setTimeout(() => {
      void refreshConversations();
    }, 250);
    return () => window.clearTimeout(timer);
  }, [searchQuery, refreshConversations]);

  // Load messages + last run when the active conversation changes.
  useEffect(() => {
    if (activeId == null) {
      setMessages([]);
      setMessagesError(null);
      setRun({ status: "idle" });
      return;
    }
    let cancelled = false;
    setMessagesLoading(true);
    setMessagesError(null);
    setStreamError(null);
    setLastStopped(false);
    listMessages(activeId)
      .then((rows) => {
        if (!cancelled) setMessages(rows);
      })
      .catch((err: unknown) => {
        if (!cancelled) setMessagesError(errorMessage(err));
      })
      .finally(() => {
        if (!cancelled) setMessagesLoading(false);
      });
    void refreshRun(activeId);
    return () => {
      cancelled = true;
    };
  }, [activeId, refreshRun]);

  const activeConv = conversations.find((c) => c.id === activeId) ?? null;
  const streaming = streamId != null;
  const modelValue: ModelValue = {
    provider_id: activeConv?.provider_id ?? defaultProviderId,
    model: activeConv?.model ?? defaultModel,
  };

  const focusPane = (pane: MobilePane) => {
    setMobilePane(pane);
    const ref = pane === "chats" ? chatsRef : conversationRef;
    requestAnimationFrame(() => ref.current?.focus());
  };

  const openInspector = () => {
    inspectorOpenerRef.current = document.activeElement as HTMLElement | null;
    setInspectorOpen(true);
    requestAnimationFrame(() => sheetRef.current?.focus());
  };

  const closeInspector = useCallback(() => {
    setInspectorOpen(false);
    const opener = inspectorOpenerRef.current;
    if (opener && document.contains(opener)) {
      requestAnimationFrame(() => opener.focus());
    } else {
      requestAnimationFrame(() => conversationRef.current?.focus());
    }
  }, []);

  const handleNav = useCallback(
    (nav: MobileNav) => {
      if (nav === "chats") {
        setCenterView("chat");
        setInspectorOpen(false);
        focusPane("chats");
      } else if (nav === "rooms") {
        setCenterView("rooms");
        setMobilePane("conversation");
        setInspectorOpen(false);
        requestAnimationFrame(() => conversationRef.current?.focus());
      } else {
        setCenterView("agents");
        setMobilePane("conversation");
        setInspectorOpen(false);
        requestAnimationFrame(() => conversationRef.current?.focus());
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );

  const activeNav: MobileNav =
    mobilePane === "chats"
      ? "chats"
      : centerView === "rooms"
        ? "rooms"
        : centerView === "agents"
          ? "agents"
          : "chats";

  const showAgents = useCallback(() => {
    setCenterView("agents");
    setMobilePane("conversation");
    setInspectorOpen(false);
    requestAnimationFrame(() => conversationRef.current?.focus());
  }, []);

  const showChat = useCallback(() => {
    setCenterView("chat");
    setMobilePane("conversation");
    setInspectorOpen(false);
    requestAnimationFrame(() => conversationRef.current?.focus());
  }, []);

  const showRooms = useCallback(() => {
    setCenterView("rooms");
    setMobilePane("conversation");
    setInspectorOpen(false);
    requestAnimationFrame(() => conversationRef.current?.focus());
  }, []);

  // Esc closes the narrow inspector sheet and returns focus.
  useEffect(() => {
    if (!isNarrow || !inspectorOpen) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        closeInspector();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [isNarrow, inspectorOpen, closeInspector]);

  // Ctrl/Cmd+K toggles the command palette, from anywhere.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setPaletteOpen((open) => !open);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  const handleNewChat = useCallback(async () => {
    const conv = await createConversation("New chat", defaultProviderId, defaultModel);
    setConversations((prev) => [conv, ...prev]);
    setActiveId(conv.id);
    setComposer("");
    setSearchQuery("");
    setCenterView("chat");
    focusPane("conversation");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [defaultProviderId, defaultModel]);

  const commands = useMemo<Command[]>(
    () => [
      {
        id: "new-chat",
        label: "New chat",
        group: "Chat",
        run: () => void handleNewChat().catch(() => undefined),
      },
      {
        id: "open-agents",
        label: "Open agents",
        group: "Chat",
        run: () => showAgents(),
      },
      {
        id: "open-rooms",
        label: "Open rooms",
        group: "Chat",
        run: () => showRooms(),
      },
      {
        id: "go-chats",
        label: "Go to: Chats",
        group: "Navigate",
        run: () => focusPane("chats"),
      },
      {
        id: "go-conversation",
        label: "Go to: Conversation",
        group: "Navigate",
        run: () => focusPane("conversation"),
      },
      {
        id: "go-inspector",
        label: "Go to: Inspector",
        group: "Navigate",
        run: () => {
          if (isNarrow) openInspector();
          else requestAnimationFrame(() => inspectorRef.current?.focus());
        },
      },
      {
        id: "open-settings",
        label: "Open settings",
        group: "Preferences",
        run: () => setSettingsOpen(true),
      },
      {
        id: "theme-light",
        label: "Theme: Light",
        group: "Preferences",
        run: () => setTheme("light"),
      },
      {
        id: "theme-dark",
        label: "Theme: Dark",
        group: "Preferences",
        run: () => setTheme("dark"),
      },
      {
        id: "theme-system",
        label: "Theme: System",
        group: "Preferences",
        run: () => setTheme("system"),
      },
    ],
    [handleNewChat, showAgents, showRooms, setTheme, isNarrow],
  );

  const onStreamEvent = useCallback(
    (convId: string) => (event: StreamEvent) => {
      if (event.type === "token") {
        setDraft((prev) => prev + event.delta);
      } else if (event.type === "done") {
        setStreamId(null);
        setDraft("");
        setLastStopped(event.stopped);
        if (event.message_id) {
          setMessages((prev) =>
            prev.some((m) => m.id === event.message_id)
              ? prev
              : [
                  ...prev,
                  {
                    id: event.message_id,
                    conversation_id: convId,
                    role: "assistant" as const,
                    content: event.content,
                    agent_id: event.agent_id,
                    created_at: Date.now(),
                  },
                ],
          );
        }
        void refreshConversations();
        void refreshRun(convId);
      } else {
        setStreamId(null);
        setDraft("");
        setStreamError(event.message);
        void refreshRun(convId);
      }
    },
    [refreshConversations, refreshRun],
  );

  const handleSend = useCallback(
    async (overrideAgentId: string | null) => {
      if (activeId == null || streaming || composer.trim() === "") return;
      const text = composer.trim();
      setComposer("");
      setStreamError(null);
      setLastStopped(false);
      const id = newStreamId();
      setStreamId(id);
      setDraft("");
      setMessages((prev) => [
        ...prev,
        {
          id: `temp-${id}`,
          conversation_id: activeId,
          role: "user" as const,
          content: text,
          agent_id: null,
          created_at: Date.now(),
        },
      ]);
      try {
        await sendChat(activeId, text, id, onStreamEvent(activeId), overrideAgentId);
        setMessages(await listMessages(activeId));
      } catch (err: unknown) {
        setStreamId(null);
        setStreamError(errorMessage(err));
        try {
          setMessages(await listMessages(activeId));
        } catch {
          // Keep the optimistic view; the error above explains the failure.
        }
      }
    },
    [activeId, streaming, composer, onStreamEvent],
  );

  const handleRegenerate = useCallback(
    async (overrideAgentId: string | null) => {
      if (activeId == null || streaming) return;
      setStreamError(null);
      setLastStopped(false);
      const id = newStreamId();
      setStreamId(id);
      setDraft("");
      try {
        await regenerateChat(activeId, id, onStreamEvent(activeId), overrideAgentId);
        setMessages(await listMessages(activeId));
      } catch (err: unknown) {
        setStreamId(null);
        setStreamError(errorMessage(err));
      }
    },
    [activeId, streaming, onStreamEvent],
  );

  const handleEditResend = useCallback(
    async (messageId: string, content: string, overrideAgentId: string | null) => {
      if (activeId == null || streaming) return;
      setStreamError(null);
      setLastStopped(false);
      const id = newStreamId();
      setStreamId(id);
      setDraft("");
      try {
        await editResendChat(
          activeId,
          messageId,
          content,
          id,
          onStreamEvent(activeId),
          overrideAgentId,
        );
        setMessages(await listMessages(activeId));
      } catch (err: unknown) {
        setStreamId(null);
        setStreamError(errorMessage(err));
      }
    },
    [activeId, streaming, onStreamEvent],
  );

  const handleStop = useCallback(() => {
    if (streamId) void cancelChat(streamId).catch(() => undefined);
  }, [streamId]);

  const handleModelChange = useCallback(
    async (value: ModelValue) => {
      if (activeId == null) return;
      const updated = await setConversationModel(
        activeId,
        value.provider_id,
        value.model,
      );
      setConversations((prev) => prev.map((c) => (c.id === updated.id ? updated : c)));
    },
    [activeId],
  );

  const handleAgentChange = useCallback(
    async (agentId: string | null) => {
      if (activeId == null) return;
      const updated = await setConversationAgent(activeId, agentId);
      setConversations((prev) => prev.map((c) => (c.id === updated.id ? updated : c)));
    },
    [activeId],
  );

  const handleRefreshModels = useCallback(async (providerId: string) => {
    setModelsLoading(true);
    try {
      const models = await providerModels(providerId);
      setModelsByProvider((prev) => ({ ...prev, [providerId]: models }));
    } catch {
      // The picker keeps whatever it had; errors surface on send/test.
    } finally {
      setModelsLoading(false);
    }
  }, []);

  const handleRename = useCallback(async (id: string, title: string) => {
    const updated = await renameConversation(id, title);
    setConversations((prev) => prev.map((c) => (c.id === updated.id ? updated : c)));
  }, []);

  const handleDelete = useCallback(async (id: string) => {
    await deleteConversation(id);
    setConversations((prev) => prev.filter((c) => c.id !== id));
    setActiveId((current) => {
      if (current === id) {
        setMessages([]);
        return null;
      }
      return current;
    });
  }, []);

  const handleProvidersChanged = useCallback(() => {
    listProviders()
      .then(setProviders)
      .catch(() => undefined);
    fetchOllamaStatus()
      .then(setOllama)
      .catch(() => undefined);
    void refreshAgents();
    // Also refresh model lists for all providers
    listProviders()
      .then((providers) => {
        Promise.allSettled(
          providers.map((p) =>
            providerModels(p.id).then((models) =>
              setModelsByProvider((prev) => ({ ...prev, [p.id]: models })),
            ),
          ),
        );
      })
      .catch(() => undefined);
    if (activeId) {
      getConversation(activeId)
        .then((conv) => {
          if (conv) {
            setConversations((prev) => prev.map((c) => (c.id === conv.id ? conv : c)));
          } else {
            setConversations((prev) => prev.filter((c) => c.id !== activeId));
            setActiveId(null);
            setMessages([]);
          }
        })
        .catch(() => undefined);
      void refreshRun(activeId);
    }
  }, [activeId, refreshAgents, refreshRun]);

  const handleDefaultModelChange = useCallback(
    (providerId: string | null, model: string) => {
      setDefaultProviderId(providerId);
      setDefaultModel(model);
      setSettings({
        "default.provider": providerId ?? "",
        "default.model": model,
      }).catch(() => undefined);
    },
    [],
  );

  const renderCenter = () => {
    if (centerView === "rooms") {
      return (
        <RoomsView
          paneRef={conversationRef}
          onOpenPalette={() => setPaletteOpen(true)}
          onBack={showChat}
          onOpenInspector={isNarrow ? openInspector : undefined}
        />
      );
    }
    if (centerView === "agents") {
      return (
        <AgentsPage
          paneRef={conversationRef}
          onOpenInspector={isNarrow ? openInspector : undefined}
          agents={agents}
          agentsLoading={agentsLoading}
          agentsError={agentsError}
          onReloadAgents={() => void refreshAgents()}
          templates={templates}
          providers={providers}
          ollama={ollama}
          modelsByProvider={modelsByProvider}
          modelsLoading={modelsLoading}
          onRefreshModels={(providerId) => void handleRefreshModels(providerId)}
          onChanged={() => void refreshAgents()}
          onOpenPalette={() => setPaletteOpen(true)}
          onBack={showChat}
        />
      );
    }
    return (
      <ConversationPane
        paneRef={conversationRef}
        onOpenInspector={isNarrow ? openInspector : undefined}
        conversation={activeConv}
        messages={messages}
        messagesLoading={messagesLoading}
        messagesError={messagesError}
        streaming={streaming}
        draft={draft}
        lastStopped={lastStopped}
        streamError={streamError}
        composer={composer}
        modelValue={modelValue}
        providers={providers}
        ollama={ollama}
        modelsByProvider={modelsByProvider}
        modelsLoading={modelsLoading}
        agents={agents}
        agentId={activeConv?.agent_id ?? null}
        onAgentChange={(id) => void handleAgentChange(id).catch(() => undefined)}
        onComposerChange={setComposer}
        onSend={(override) => void handleSend(override)}
        onStop={handleStop}
        onRegenerate={(override) => void handleRegenerate(override)}
        onEditResend={(messageId, content, override) =>
          void handleEditResend(messageId, content, override)
        }
        onModelChange={(value) => void handleModelChange(value).catch(() => undefined)}
        onRefreshModels={(providerId) => void handleRefreshModels(providerId)}
        onNewChat={() => void handleNewChat().catch(() => undefined)}
        onOpenPalette={() => setPaletteOpen(true)}
        onOpenSettings={() => setSettingsOpen(true)}
        onShowAgents={showAgents}
      />
    );
  };

  const renderPanes = () => {
    const sidebar = (
      <Sidebar
        paneRef={chatsRef}
        conversations={conversations}
        activeId={activeId}
        loading={chatsLoading}
        loadError={chatsError}
        onReload={() => void refreshConversations()}
        onNew={() => void handleNewChat().catch(() => undefined)}
        onSelect={(id) => {
          setActiveId(id);
          setCenterView("chat");
          if (isNarrow) focusPane("conversation");
        }}
        onRename={handleRename}
        onDelete={handleDelete}
        onSearch={setSearchQuery}
        searchQuery={searchQuery}
        onOpenPalette={() => setPaletteOpen(true)}
        onOpenSettings={() => setSettingsOpen(true)}
        onShowAgents={showAgents}
        onShowRooms={showRooms}
      />
    );
    if (isNarrow) {
      if (mobilePane === "chats") return sidebar;
      return renderCenter();
    }
    return (
      <>
        {sidebar}
        {renderCenter()}
        <Inspector
          paneRef={inspectorRef}
          onOpenSettings={() => setSettingsOpen(true)}
          appInfo={appInfo}
          conversationTitle={activeConv?.title ?? null}
          run={run}
          agents={agents}
          providers={providers}
        />
      </>
    );
  };

  return (
    <div className={`app${isNarrow ? " app--narrow" : ""}`}>
      <div className="app__panes">{renderPanes()}</div>
      {isNarrow && inspectorOpen && (
        <div
          className="p4-sheet-backdrop"
          onClick={(e) => {
            if (e.target === e.currentTarget) closeInspector();
          }}
        >
          <div
            className="p4-sheet"
            role="dialog"
            aria-modal="true"
            aria-label="Inspector"
            ref={sheetRef}
            tabIndex={-1}
            onKeyDown={(e) => {
              if (e.key !== "Tab") return;
              const el = sheetRef.current;
              if (!el) return;
              const focusable = Array.from(
                el.querySelectorAll<HTMLElement>(
                  'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
                ),
              ).filter((n) => !n.hasAttribute("disabled") && n.offsetParent !== null);
              if (focusable.length === 0) return;
              const first = focusable[0];
              const last = focusable[focusable.length - 1];
              const active = document.activeElement;
              if (e.shiftKey && (active === first || !el.contains(active))) {
                e.preventDefault();
                last.focus();
              } else if (!e.shiftKey && active === last) {
                e.preventDefault();
                first.focus();
              }
            }}
          >
            <div className="p4-sheet__handle" aria-hidden="true" />
            <div className="p4-sheet__bar">
              <span className="p4-sheet__title">Inspector</span>
              <button
                type="button"
                className="btn btn--icon"
                onClick={closeInspector}
                aria-label="Close inspector"
              >
                <IconClose />
              </button>
            </div>
            <div className="p4-sheet__content">
              <Inspector
                paneRef={inspectorRef}
                onOpenSettings={() => setSettingsOpen(true)}
                appInfo={appInfo}
                conversationTitle={activeConv?.title ?? null}
                run={run}
                agents={agents}
                providers={providers}
              />
            </div>
          </div>
        </div>
      )}
      {isNarrow && <BottomNav active={activeNav} onSelect={handleNav} />}
      <CommandPalette
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        commands={commands}
      />
      <SettingsDialog
        open={settingsOpen}
        onClose={() => setSettingsOpen(false)}
        theme={theme}
        onThemeChange={(choice: ThemeChoice) => setTheme(choice)}
        defaultProviderId={defaultProviderId}
        defaultModel={defaultModel}
        onDefaultModelChange={handleDefaultModelChange}
        onProvidersChanged={handleProvidersChanged}
      />
    </div>
  );
}
