import { useCallback, useEffect, useState } from "react";
import { errorMessage } from "../lib/tauri";
import {
  NO_PERMISSIONS,
  agentPermissions,
  agentTools,
  mcpServers,
  mcpTools,
  setAgentPermissions,
  setAgentTools,
  type Agent,
  type AgentPermissions,
  type McpServer,
  type McpTool,
} from "../lib/api";

export const PERM_HINTS: Record<keyof AgentPermissions, string> = {
  workspace: "Read and write the workspace folder.",
  network: "Allow outbound network access.",
  shell: "Allow running shell commands.",
};

function isNoAccess(tools: string[], perms: AgentPermissions): boolean {
  return (
    tools.length === 0 && !perms.workspace && !perms.network && !perms.shell
  );
}

/** Full "Tools & permissions" block for the agent builder (desktop only). */
export function AgentToolsPermissions({ agentId }: { agentId: string }) {
  const [servers, setServers] = useState<McpServer[]>([]);
  const [toolsByServer, setToolsByServer] = useState<Record<string, McpTool[]>>({});
  const [selected, setSelected] = useState<string[]>([]);
  const [perms, setPerms] = useState<AgentPermissions>({ ...NO_PERMISSIONS });
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [reloadKey, setReloadKey] = useState(0);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [serverRows, toolIds, permRows] = await Promise.all([
        mcpServers(),
        agentTools(agentId),
        agentPermissions(agentId),
      ]);
      setServers(serverRows);
      setSelected(toolIds);
      setPerms(permRows);
      const grouped: Record<string, McpTool[]> = {};
      await Promise.all(
        serverRows.map(async (s) => {
          try {
            grouped[s.id] = await mcpTools(s.id);
          } catch {
            grouped[s.id] = [];
          }
        }),
      );
      setToolsByServer(grouped);
    } catch (err: unknown) {
      setError(errorMessage(err));
    } finally {
      setLoading(false);
    }
  }, [agentId]);

  useEffect(() => {
    void load();
  }, [load, reloadKey]);

  const toggleTool = async (id: string, on: boolean) => {
    const next = on ? [...selected, id] : selected.filter((t) => t !== id);
    setSelected(next);
    setSaving(true);
    setError(null);
    try {
      setSelected(await setAgentTools(agentId, next));
    } catch (err: unknown) {
      setError(errorMessage(err));
      setSelected(selected);
    } finally {
      setSaving(false);
    }
  };

  const togglePerm = async (key: keyof AgentPermissions, on: boolean) => {
    const next = { ...perms, [key]: on };
    setPerms(next);
    setSaving(true);
    setError(null);
    try {
      setPerms(await setAgentPermissions(agentId, next));
    } catch (err: unknown) {
      setError(errorMessage(err));
      setPerms(perms);
    } finally {
      setSaving(false);
    }
  };

  if (loading) {
    return (
      <p className="muted" role="status">
        <span className="spinner" aria-hidden="true" /> Loading tools…
      </p>
    );
  }

  if (error && servers.length === 0 && selected.length === 0) {
    return (
      <div className="notice notice--error" role="alert">
        <p className="notice__title">Couldn’t load tools for this agent.</p>
        <p className="notice__body">{error}</p>
        <p className="notice__body">
          <button type="button" className="btn" onClick={() => setReloadKey((k) => k + 1)}>
            Retry
          </button>
        </p>
      </div>
    );
  }

  return (
    <div className="p4-stack">
      {error && (
        <div className="notice notice--error" role="alert">
          <p className="notice__body">{error}</p>
          <p className="notice__body">
            <button type="button" className="btn" onClick={() => setReloadKey((k) => k + 1)}>
              Retry
            </button>
          </p>
        </div>
      )}
      {servers.length === 0 ? (
        <p className="muted">No MCP servers yet — add one in Settings → Tools.</p>
      ) : (
        servers.map((server) => {
          const tools = toolsByServer[server.id] ?? [];
          if (tools.length === 0) return null;
          return (
            <fieldset key={server.id} className="p4-fieldset">
              <legend className="settings__label">{server.name}</legend>
              <ul className="p4-checklist">
                {tools.map((tool) => {
                  const id = `${server.id}:${tool.name}`;
                  return (
                    <li key={id}>
                      <label className="check-row" title={tool.description}>
                        <input
                          type="checkbox"
                          checked={selected.includes(id)}
                          disabled={saving}
                          onChange={(e) => void toggleTool(id, e.target.checked)}
                        />
                        <code>{tool.name}</code>
                      </label>
                    </li>
                  );
                })}
              </ul>
            </fieldset>
          );
        })
      )}
      <fieldset className="p4-fieldset">
        <legend className="settings__label">Permissions</legend>
        <ul className="p4-checklist">
          {(Object.keys(PERM_HINTS) as Array<keyof AgentPermissions>).map((key) => (
            <li key={key}>
              <label className="check-row">
                <input
                  type="checkbox"
                  checked={perms[key]}
                  disabled={saving}
                  onChange={(e) => void togglePerm(key, e.target.checked)}
                />
                <span>
                  <strong>{key}</strong> — {PERM_HINTS[key]}
                </span>
              </label>
            </li>
          ))}
        </ul>
      </fieldset>
      {isNoAccess(selected, perms) && (
        <p className="muted" role="note">
          No extra access — this agent can only chat.
        </p>
      )}
    </div>
  );
}

/** Compact three-checkbox permissions row for RoomForm's member editor. */
export function MemberPermissionsRow({ agent }: { agent: Agent }) {
  const [perms, setPerms] = useState<AgentPermissions>(
    () => agent.permissions ?? { ...NO_PERMISSIONS },
  );
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    setPerms(agent.permissions ?? { ...NO_PERMISSIONS });
    setError(null);
  }, [agent]);

  const toggle = async (key: keyof AgentPermissions, on: boolean) => {
    const next = { ...perms, [key]: on };
    setPerms(next);
    setBusy(true);
    setError(null);
    try {
      setPerms(await setAgentPermissions(agent.id, next));
    } catch (err: unknown) {
      setError(errorMessage(err));
      setPerms(perms);
    } finally {
      setBusy(false);
    }
  };

  const none = !perms.workspace && !perms.network && !perms.shell;

  return (
    <span className="p4-member-perms">
      <span className="p4-member-perms__checks">
        {(Object.keys(PERM_HINTS) as Array<keyof AgentPermissions>).map((key) => (
          <label key={key} className="check-row check-row--sm" title={PERM_HINTS[key]}>
            <input
              type="checkbox"
              aria-label={`${key} permission for ${agent.name}`}
              checked={perms[key]}
              disabled={busy}
              onChange={(e) => void toggle(key, e.target.checked)}
            />
            {key}
          </label>
        ))}
      </span>
      {none ? (
        <span className="muted p4-member-perms__hint">no extra access</span>
      ) : null}
      {error && (
        <span className="notice notice--error" role="alert">
          <span className="notice__body">{error}</span>
        </span>
      )}
    </span>
  );
}
