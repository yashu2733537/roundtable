import { useMemo, useRef, useState } from "react";
import type { RefObject } from "react";
import EmptyState from "./EmptyState";
import ModelPicker from "./ModelPicker";
import { AgentToolsPermissions } from "./AgentAccess";
import { IconCommand, IconPanelDetail } from "./Icon";
import { useIsAndroid } from "./useAndroid";
import {
  cancelChat,
  createAgent,
  deleteAgent,
  duplicateAgent,
  errorMessage,
  exportAgent,
  importAgent,
  newStreamId,
  testAgent,
  updateAgent,
  type Agent,
  type AgentTemplate,
  type NewAgent,
  type OllamaStatus,
  type ProviderInfo,
  type StreamEvent,
} from "../lib/api";

interface AgentsPageProps {
  paneRef: RefObject<HTMLElement>;
  agents: Agent[];
  agentsLoading: boolean;
  agentsError: string | null;
  onReloadAgents: () => void;
  templates: AgentTemplate[];
  providers: ProviderInfo[];
  ollama: OllamaStatus;
  modelsByProvider: Record<string, string[]>;
  modelsLoading: boolean;
  onRefreshModels: (providerId: string) => void;
  onChanged: () => void;
  onOpenPalette: () => void;
  onBack: () => void;
  onOpenInspector?: () => void;
}

type StatusFilter = "all" | "enabled" | "disabled";

function blankDraft(): NewAgent {
  return {
    name: "",
    avatar_color: "#2c5fd0",
    avatar_icon: "",
    role: "",
    system_prompt: "",
    provider_id: null,
    model: "",
    temperature: 0.7,
    max_tokens: 0,
    enabled: true,
  };
}

function draftFromTemplate(template: AgentTemplate): NewAgent {
  return {
    name: template.name,
    avatar_color: template.avatar_color,
    avatar_icon: template.avatar_icon,
    role: template.role,
    system_prompt: template.system_prompt,
    provider_id: null,
    model: template.model,
    temperature: template.temperature,
    max_tokens: template.max_tokens,
    enabled: true,
  };
}

function draftFromAgent(agent: Agent): NewAgent {
  return {
    name: agent.name,
    avatar_color: agent.avatar_color,
    avatar_icon: agent.avatar_icon,
    role: agent.role,
    system_prompt: agent.system_prompt,
    provider_id: agent.provider_id,
    model: agent.model,
    temperature: agent.temperature,
    max_tokens: agent.max_tokens,
    enabled: agent.enabled,
  };
}

export function AgentAvatar({
  agent,
  size = 28,
}: {
  agent: Pick<Agent, "avatar_color" | "avatar_icon" | "name">;
  size?: number;
}) {
  const icon = agent.avatar_icon.trim() || agent.name.trim().charAt(0) || "?";
  return (
    <span
      className="avatar"
      style={{
        backgroundColor: agent.avatar_color,
        width: size,
        height: size,
        fontSize: Math.round(size * 0.45),
      }}
      aria-hidden="true"
    >
      {icon}
    </span>
  );
}

interface TestTurn {
  role: "user" | "assistant";
  content: string;
}

export default function AgentsPage({
  paneRef,
  agents,
  agentsLoading,
  agentsError,
  onReloadAgents,
  templates,
  providers,
  ollama,
  modelsByProvider,
  modelsLoading,
  onRefreshModels,
  onChanged,
  onOpenPalette,
  onBack,
  onOpenInspector,
}: AgentsPageProps) {
  const [search, setSearch] = useState("");
  const [statusFilter, setStatusFilter] = useState<StatusFilter>("all");
  const [providerFilter, setProviderFilter] = useState<string>("all");
  const [editingId, setEditingId] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [draft, setDraft] = useState<NewAgent>(blankDraft);
  const [formError, setFormError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [pendingDeleteId, setPendingDeleteId] = useState<string | null>(null);
  const [rowBusy, setRowBusy] = useState(false);

  // Test mini chat (local transcript, never persisted).
  const [testInput, setTestInput] = useState("");
  const [testTranscript, setTestTranscript] = useState<TestTurn[]>([]);
  const [testStreamId, setTestStreamId] = useState<string | null>(null);
  const [testDraft, setTestDraft] = useState("");
  const [testError, setTestError] = useState<string | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  const isAndroid = useIsAndroid();

  const filtered = useMemo(() => {
    const query = search.trim().toLowerCase();
    return agents.filter((agent) => {
      if (statusFilter === "enabled" && !agent.enabled) return false;
      if (statusFilter === "disabled" && agent.enabled) return false;
      if (providerFilter !== "all" && (agent.provider_id ?? "") !== providerFilter) {
        return false;
      }
      if (query === "") return true;
      return (
        agent.name.toLowerCase().includes(query) ||
        agent.role.toLowerCase().includes(query)
      );
    });
  }, [agents, search, statusFilter, providerFilter]);

  const builderOpen = creating || editingId !== null;
  const providerName = (id: string | null) => {
    if (id == null) return "—";
    if (id === "ollama") return "Ollama (local)";
    return providers.find((p) => p.id === id)?.name ?? "Removed provider";
  };

  const startCreate = (initial?: NewAgent) => {
    setEditingId(null);
    setCreating(true);
    setDraft(initial ?? blankDraft());
    setFormError(null);
    setTestTranscript([]);
    setTestDraft("");
    setTestError(null);
  };

  const startEdit = (agent: Agent) => {
    setCreating(false);
    setEditingId(agent.id);
    setDraft(draftFromAgent(agent));
    setFormError(null);
    setTestTranscript([]);
    setTestDraft("");
    setTestError(null);
    setPendingDeleteId(null);
  };

  const closeBuilder = () => {
    setCreating(false);
    setEditingId(null);
    setFormError(null);
    if (testStreamId) void cancelChat(testStreamId).catch(() => undefined);
    setTestStreamId(null);
  };

  const save = async () => {
    setSaving(true);
    setFormError(null);
    try {
      if (editingId) {
        await updateAgent(editingId, draft);
      } else {
        const created = await createAgent(draft);
        setEditingId(created.id);
        setCreating(false);
      }
      onChanged();
    } catch (err: unknown) {
      setFormError(errorMessage(err));
    } finally {
      setSaving(false);
    }
  };

  const remove = async (id: string) => {
    setRowBusy(true);
    try {
      await deleteAgent(id);
      setPendingDeleteId(null);
      if (editingId === id) closeBuilder();
      onChanged();
    } catch (err: unknown) {
      setFormError(errorMessage(err));
    } finally {
      setRowBusy(false);
    }
  };

  const duplicate = async (id: string) => {
    setRowBusy(true);
    try {
      const copy = await duplicateAgent(id);
      onChanged();
      startEdit(copy);
    } catch (err: unknown) {
      setFormError(errorMessage(err));
    } finally {
      setRowBusy(false);
    }
  };

  const exportOne = async (agent: Agent) => {
    try {
      const json = await exportAgent(agent.id);
      const blob = new Blob([json], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = url;
      link.download = `agent-${agent.name.toLowerCase().replace(/[^a-z0-9]+/g, "-")}.json`;
      document.body.appendChild(link);
      link.click();
      document.body.removeChild(link);
      URL.revokeObjectURL(url);
    } catch (err: unknown) {
      setFormError(errorMessage(err));
    }
  };

  const onImportFile = async (file: File | undefined) => {
    if (!file) return;
    try {
      const imported = await importAgent(await file.text());
      onChanged();
      startEdit(imported);
    } catch (err: unknown) {
      setFormError(errorMessage(err));
    }
  };

  const sendTest = async () => {
    if (testInput.trim() === "" || testStreamId) return;
    const text = testInput.trim();
    setTestInput("");
    setTestError(null);
    setTestTranscript((prev) => [...prev, { role: "user", content: text }]);
    const id = newStreamId();
    setTestStreamId(id);
    setTestDraft("");
    try {
      await testAgent(draft, text, id, (event: StreamEvent) => {
        if (event.type === "token") {
          setTestDraft((prev) => prev + event.delta);
        } else if (event.type === "done") {
          setTestStreamId(null);
          const content = event.content || testDraftRef.current;
          if (content) {
            setTestTranscript((prev) => [...prev, { role: "assistant", content }]);
          }
          setTestDraft("");
        } else {
          setTestStreamId(null);
          setTestDraft("");
          setTestError(event.message);
        }
      });
    } catch (err: unknown) {
      setTestStreamId(null);
      setTestError(errorMessage(err));
    }
  };

  // The Done handler above closes over a stale draft; mirror it in a ref.
  const testDraftRef = useRef("");
  testDraftRef.current = testDraft;

  const stopTest = () => {
    if (testStreamId) void cancelChat(testStreamId).catch(() => undefined);
  };

  return (
    <main
      className="pane pane--conversation"
      ref={paneRef}
      tabIndex={-1}
      aria-label="Agents"
    >
      <header className="pane__header">
        <h1 className="pane__title">Agents</h1>
        <div className="pane__header-actions">
          <button type="button" className="btn" onClick={onBack}>
            Back to chat
          </button>
          {onOpenInspector && (
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

      <div className="pane__body agents">
        <div className="agents__toolbar">
          <input
            type="search"
            className="input"
            placeholder="Search agents…"
            aria-label="Search agents"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
          <select
            className="select"
            aria-label="Filter by status"
            value={statusFilter}
            onChange={(e) => setStatusFilter(e.target.value as StatusFilter)}
          >
            <option value="all">All statuses</option>
            <option value="enabled">Enabled</option>
            <option value="disabled">Disabled</option>
          </select>
          <select
            className="select"
            aria-label="Filter by provider"
            value={providerFilter}
            onChange={(e) => setProviderFilter(e.target.value)}
          >
            <option value="all">All providers</option>
            <option value="">No provider</option>
            {providers.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
            <option value="ollama">Ollama (local)</option>
          </select>
        </div>

        <div className="agents__actions">
          <button
            type="button"
            className="btn btn--primary"
            onClick={() => startCreate()}
          >
            New agent
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => fileRef.current?.click()}
          >
            Import…
          </button>
          <input
            ref={fileRef}
            type="file"
            accept="application/json,.json"
            hidden
            aria-label="Choose an agent file to import"
            onChange={(e) => {
              void onImportFile(e.target.files?.[0]);
              e.target.value = "";
            }}
          />
        </div>

        {agentsLoading && (
          <p className="muted" role="status">
            <span className="spinner" aria-hidden="true" /> Loading agents…
          </p>
        )}
        {agentsError && (
          <div className="notice notice--error" role="alert">
            <p className="notice__title">Couldn’t load agents.</p>
            <p className="notice__body">{agentsError}</p>
            <p className="notice__body">
              <button type="button" className="btn" onClick={onReloadAgents}>
                Retry
              </button>
            </p>
          </div>
        )}

        {!agentsLoading && !agentsError && filtered.length === 0 && (
          <EmptyState
            title={agents.length === 0 ? "No agents yet" : "No matches"}
            description={
              agents.length === 0
                ? "Create one from scratch or start from a role template below."
                : "Nothing matches those filters."
            }
          />
        )}

        <ul className="agents__list">
          {filtered.map((agent) => (
            <li
              key={agent.id}
              className={`agent-row${editingId === agent.id ? " agent-row--active" : ""}${
                agent.enabled ? "" : " agent-row--disabled"
              }`}
            >
              <button
                type="button"
                className="agent-row__main"
                onClick={() => startEdit(agent)}
                aria-label={`Edit ${agent.name}`}
              >
                <AgentAvatar agent={agent} />
                <span className="agent-row__text">
                  <span className="agent-row__name">
                    {agent.name}
                    {!agent.enabled && (
                      <span className="agent-row__off"> (disabled)</span>
                    )}
                  </span>
                  <span className="agent-row__sub">
                    {agent.role || "No role description"} ·{" "}
                    {providerName(agent.provider_id)}
                    {agent.model ? ` · ${agent.model}` : ""}
                  </span>
                </span>
              </button>
              <span className="agent-row__actions">
                <button
                  type="button"
                  className="btn btn--sm"
                  disabled={rowBusy}
                  onClick={() => void duplicate(agent.id)}
                >
                  Duplicate
                </button>
                <button
                  type="button"
                  className="btn btn--sm"
                  onClick={() => void exportOne(agent)}
                >
                  Export
                </button>
                {pendingDeleteId === agent.id ? (
                  <button
                    type="button"
                    className="btn btn--danger btn--sm"
                    disabled={rowBusy}
                    onClick={() => void remove(agent.id)}
                  >
                    Confirm?
                  </button>
                ) : (
                  <button
                    type="button"
                    className="btn btn--sm"
                    onClick={() => {
                      setPendingDeleteId(agent.id);
                      window.setTimeout(() => {
                        setPendingDeleteId((current) =>
                          current === agent.id ? null : current,
                        );
                      }, 4000);
                    }}
                  >
                    Delete
                  </button>
                )}
              </span>
            </li>
          ))}
        </ul>

        {!builderOpen && (
          <section className="card" aria-labelledby="templates-heading">
            <h3 className="card__title" id="templates-heading">
              Start from a template
            </h3>
            <ul className="chip-list">
              {templates.map((template) => (
                <li key={template.key}>
                  <button
                    type="button"
                    className="btn btn--sm"
                    title={template.role}
                    onClick={() => startCreate(draftFromTemplate(template))}
                  >
                    {template.name}
                  </button>
                </li>
              ))}
            </ul>
          </section>
        )}

        {builderOpen && (
          <section className="card builder" aria-labelledby="builder-heading">
            <h3 className="card__title" id="builder-heading">
              {editingId ? "Edit agent" : "New agent"}
            </h3>

            <div className="builder__grid">
              <div className="settings__field">
                <label className="settings__label" htmlFor="agent-name">
                  Name
                </label>
                <input
                  id="agent-name"
                  className="input"
                  value={draft.name}
                  onChange={(e) => setDraft({ ...draft, name: e.target.value })}
                  autoComplete="off"
                />
              </div>
              <div className="settings__field">
                <label className="settings__label" htmlFor="agent-role">
                  Role description
                </label>
                <input
                  id="agent-role"
                  className="input"
                  placeholder="What this agent is for"
                  value={draft.role}
                  onChange={(e) => setDraft({ ...draft, role: e.target.value })}
                  autoComplete="off"
                />
              </div>
              <div className="settings__field">
                <label className="settings__label" htmlFor="agent-color">
                  Avatar color
                </label>
                <div className="settings__key-row">
                  <input
                    id="agent-color"
                    type="color"
                    className="color-input"
                    aria-label="Avatar color picker"
                    value={/^#[0-9a-fA-F]{6}$/.test(draft.avatar_color) ? draft.avatar_color : "#2c5fd0"}
                    onChange={(e) => setDraft({ ...draft, avatar_color: e.target.value })}
                  />
                  <input
                    className="input"
                    aria-label="Avatar color hex"
                    value={draft.avatar_color}
                    onChange={(e) => setDraft({ ...draft, avatar_color: e.target.value })}
                    autoComplete="off"
                    spellCheck={false}
                  />
                </div>
              </div>
              <div className="settings__field">
                <label className="settings__label" htmlFor="agent-icon">
                  Avatar icon
                </label>
                <input
                  id="agent-icon"
                  className="input"
                  placeholder="One or two characters"
                  value={draft.avatar_icon}
                  onChange={(e) => setDraft({ ...draft, avatar_icon: e.target.value })}
                  autoComplete="off"
                />
              </div>
            </div>

            <div className="settings__field">
              <label className="settings__label" htmlFor="agent-system">
                System prompt
              </label>
              <textarea
                id="agent-system"
                className="textarea"
                rows={6}
                value={draft.system_prompt}
                onChange={(e) => setDraft({ ...draft, system_prompt: e.target.value })}
                spellCheck={false}
              />
            </div>

            <div className="builder__preview" aria-live="polite">
              <span className="builder__preview-label">Live preview</span>
              <div className="msg msg--assistant">
                <div className="msg__head">
                  <span className="msg__role">
                    <AgentAvatar agent={{ ...draft, name: draft.name || "Agent" }} size={20} />{" "}
                    {draft.name || "Unnamed agent"}
                  </span>
                </div>
                <p className="msg__text">
                  <span className="muted">System: </span>
                  {draft.system_prompt || "…"}
                </p>
              </div>
            </div>

            <div className="settings__field">
              <span className="settings__label" id="agent-model-label">
                Provider + model
              </span>
              <div role="group" aria-labelledby="agent-model-label">
                <ModelPicker
                  providers={providers}
                  ollama={ollama}
                  modelsByProvider={modelsByProvider}
                  modelsLoading={modelsLoading}
                  value={{ provider_id: draft.provider_id, model: draft.model }}
                  onChange={(value) =>
                    setDraft({
                      ...draft,
                      provider_id: value.provider_id,
                      model: value.model,
                    })
                  }
                  onRefreshModels={onRefreshModels}
                />
              </div>
            </div>

            <div className="builder__grid">
              <div className="settings__field">
                <label className="settings__label" htmlFor="agent-temp">
                  Temperature: {draft.temperature.toFixed(1)}
                </label>
                <input
                  id="agent-temp"
                  type="range"
                  className="slider"
                  min={0}
                  max={2}
                  step={0.1}
                  value={draft.temperature}
                  onChange={(e) =>
                    setDraft({ ...draft, temperature: Number(e.target.value) })
                  }
                />
              </div>
              <div className="settings__field">
                <label className="settings__label" htmlFor="agent-tokens">
                  Max tokens: {draft.max_tokens === 0 ? "auto" : draft.max_tokens}
                </label>
                <input
                  id="agent-tokens"
                  type="range"
                  className="slider"
                  min={0}
                  max={8000}
                  step={256}
                  value={draft.max_tokens}
                  onChange={(e) =>
                    setDraft({ ...draft, max_tokens: Number(e.target.value) })
                  }
                />
              </div>
            </div>

            <label className="check-row">
              <input
                type="checkbox"
                checked={draft.enabled}
                onChange={(e) => setDraft({ ...draft, enabled: e.target.checked })}
              />
              Enabled (disabled agents stay listed but never answer)
            </label>

            {!isAndroid && (
              <div className="settings__field">
                <span className="settings__label" id="agent-tools-heading">
                  Tools & permissions
                </span>
                <div role="group" aria-labelledby="agent-tools-heading">
                  {editingId ? (
                    <AgentToolsPermissions agentId={editingId} />
                  ) : (
                    <p className="settings__hint">
                      Save the agent first to manage its tools and permissions.
                    </p>
                  )}
                </div>
              </div>
            )}

            {formError && (
              <div className="notice notice--error" role="alert">
                <p className="notice__body">{formError}</p>
              </div>
            )}

            <div className="provider-form__actions">
              <button
                type="button"
                className="btn btn--primary"
                disabled={saving}
                onClick={() => void save()}
              >
                {saving ? "Saving…" : editingId ? "Save" : "Create"}
              </button>
              <button type="button" className="btn" onClick={closeBuilder}>
                Cancel
              </button>
            </div>

            <div className="builder__test">
              <h4 className="settings__label">Test agent</h4>
              <ol className="chat__list">
                {testTranscript.map((turn, index) => (
                  <li key={index} className={`msg msg--${turn.role}`}>
                    <div className="msg__head">
                      <span className="msg__role">
                        {turn.role === "user" ? "You" : draft.name || "Agent"}
                      </span>
                    </div>
                    <p className="msg__text">{turn.content}</p>
                  </li>
                ))}
                {testStreamId && (
                  <li className="msg msg--assistant" aria-live="polite">
                    <div className="msg__head">
                      <span className="msg__role">{draft.name || "Agent"}</span>
                    </div>
                    <p className="msg__text">
                      {testDraft}
                      <span className="chat__cursor" aria-hidden="true" />
                    </p>
                  </li>
                )}
              </ol>
              {testError && (
                <div className="notice notice--error" role="alert">
                  <p className="notice__title">The test turn failed.</p>
                  <p className="notice__body">{testError}</p>
                </div>
              )}
              <form
                className="chat__composer"
                onSubmit={(e) => {
                  e.preventDefault();
                  if (!testStreamId) void sendTest();
                }}
              >
                <textarea
                  className="textarea chat__input"
                  aria-label="Test message"
                  placeholder="Write a test message… (Enter sends, Shift+Enter adds a line)"
                  value={testInput}
                  onChange={(e) => setTestInput(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && !e.shiftKey) {
                      e.preventDefault();
                      if (!testStreamId) void sendTest();
                    }
                  }}
                  rows={2}
                  disabled={testStreamId !== null}
                />
                {testStreamId ? (
                  <button
                    type="button"
                    className="btn btn--danger chat__send"
                    onClick={stopTest}
                  >
                    Stop
                  </button>
                ) : (
                  <button
                    type="submit"
                    className="btn btn--primary chat__send"
                    disabled={testInput.trim() === ""}
                  >
                    Send
                  </button>
                )}
              </form>
              <p className="settings__hint">
                Test turns are not saved anywhere.
              </p>
            </div>
          </section>
        )}
      </div>
    </main>
  );
}
