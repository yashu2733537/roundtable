import { useCallback, useEffect, useState } from "react";
import { errorMessage } from "../lib/tauri";
import {
  clearWorkspaceDir,
  hasWebSearchKey,
  mcpAddServer,
  mcpRemoveServer,
  mcpServers,
  mcpSetEnabled,
  mcpTools,
  pickWorkspaceDir,
  setWebSearchKey,
  workspaceDir,
  type McpServer,
  type McpTool,
} from "../lib/api";

type Transport = "stdio" | "http";

export default function ToolsSettings() {
  const [dir, setDir] = useState<string | null>(null);
  const [dirLoading, setDirLoading] = useState(true);
  const [dirError, setDirError] = useState<string | null>(null);
  const [dirBusy, setDirBusy] = useState(false);

  const [keySaved, setKeySaved] = useState(false);
  const [keyLoading, setKeyLoading] = useState(true);
  const [keyInput, setKeyInput] = useState("");
  const [keyBusy, setKeyBusy] = useState(false);
  const [keyError, setKeyError] = useState<string | null>(null);

  const [servers, setServers] = useState<McpServer[]>([]);
  const [serversLoading, setServersLoading] = useState(true);
  const [serversError, setServersError] = useState<string | null>(null);
  const [rowBusy, setRowBusy] = useState(false);
  const [pendingDeleteId, setPendingDeleteId] = useState<string | null>(null);

  const [name, setName] = useState("");
  const [transport, setTransport] = useState<Transport>("stdio");
  const [command, setCommand] = useState("");
  const [argsText, setArgsText] = useState("");
  const [url, setUrl] = useState("");
  const [addBusy, setAddBusy] = useState(false);
  const [addError, setAddError] = useState<string | null>(null);

  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [toolsByServer, setToolsByServer] = useState<Record<string, McpTool[]>>({});
  const [toolsLoadingId, setToolsLoadingId] = useState<string | null>(null);
  const [toolsError, setToolsError] = useState<string | null>(null);

  const loadDir = useCallback(async () => {
    setDirLoading(true);
    setDirError(null);
    try {
      setDir(await workspaceDir());
    } catch (err: unknown) {
      setDirError(errorMessage(err));
    } finally {
      setDirLoading(false);
    }
  }, []);

  const loadServers = useCallback(async () => {
    setServersLoading(true);
    setServersError(null);
    try {
      setServers(await mcpServers());
    } catch (err: unknown) {
      setServersError(errorMessage(err));
    } finally {
      setServersLoading(false);
    }
  }, []);

  const loadKey = useCallback(async () => {
    setKeyLoading(true);
    setKeyError(null);
    try {
      setKeySaved(await hasWebSearchKey());
    } catch (err: unknown) {
      setKeyError(errorMessage(err));
    } finally {
      setKeyLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadDir();
    void loadServers();
    void loadKey();
  }, [loadDir, loadServers, loadKey]);

  const doSaveKey = async () => {
    if (keyInput.trim() === "") return;
    setKeyBusy(true);
    setKeyError(null);
    try {
      await setWebSearchKey(keyInput.trim());
      setKeyInput("");
      setKeySaved(true);
    } catch (err: unknown) {
      setKeyError(errorMessage(err));
    } finally {
      setKeyBusy(false);
    }
  };

  const doClearKey = async () => {
    setKeyBusy(true);
    setKeyError(null);
    try {
      await setWebSearchKey("");
      setKeySaved(false);
    } catch (err: unknown) {
      setKeyError(errorMessage(err));
    } finally {
      setKeyBusy(false);
    }
  };

  const doPick = async () => {
    setDirBusy(true);
    setDirError(null);
    try {
      const picked = await pickWorkspaceDir();
      if (picked != null) setDir(picked);
    } catch (err: unknown) {
      setDirError(errorMessage(err));
    } finally {
      setDirBusy(false);
    }
  };

  const doClear = async () => {
    setDirBusy(true);
    setDirError(null);
    try {
      await clearWorkspaceDir();
      setDir(null);
    } catch (err: unknown) {
      setDirError(errorMessage(err));
    } finally {
      setDirBusy(false);
    }
  };

  const doToggle = async (server: McpServer) => {
    setRowBusy(true);
    try {
      const next = await mcpSetEnabled(server.id, !server.enabled);
      setServers((prev) => prev.map((s) => (s.id === server.id ? next : s)));
    } catch (err: unknown) {
      setServersError(errorMessage(err));
    } finally {
      setRowBusy(false);
    }
  };

  const doRemove = async (id: string) => {
    setRowBusy(true);
    try {
      await mcpRemoveServer(id);
      setPendingDeleteId(null);
      setServers((prev) => prev.filter((s) => s.id !== id));
      if (expandedId === id) setExpandedId(null);
    } catch (err: unknown) {
      setServersError(errorMessage(err));
    } finally {
      setRowBusy(false);
    }
  };

  const doAdd = async () => {
    const trimmedName = name.trim();
    if (trimmedName === "") {
      setAddError("Give the server a name.");
      return;
    }
    if (transport === "stdio" && command.trim() === "") {
      setAddError("Stdio servers need a command to launch.");
      return;
    }
    if (transport === "http" && url.trim() === "") {
      setAddError("HTTP servers need a URL.");
      return;
    }
    setAddBusy(true);
    setAddError(null);
    try {
      const created =
        transport === "stdio"
          ? await mcpAddServer({
              name: trimmedName,
              transport,
              command: command.trim(),
              args: argsText
                .split(/\s+/)
                .map((a) => a.trim())
                .filter((a) => a !== ""),
            })
          : await mcpAddServer({ name: trimmedName, transport, url: url.trim() });
      setServers((prev) => [...prev, created]);
      setName("");
      setCommand("");
      setArgsText("");
      setUrl("");
    } catch (err: unknown) {
      setAddError(errorMessage(err));
    } finally {
      setAddBusy(false);
    }
  };

  const toggleTools = async (server: McpServer) => {
    if (expandedId === server.id) {
      setExpandedId(null);
      return;
    }
    setExpandedId(server.id);
    if (toolsByServer[server.id]) return;
    setToolsLoadingId(server.id);
    setToolsError(null);
    try {
      const tools = await mcpTools(server.id);
      setToolsByServer((prev) => ({ ...prev, [server.id]: tools }));
    } catch (err: unknown) {
      setToolsError(errorMessage(err));
    } finally {
      setToolsLoadingId(null);
    }
  };

  return (
    <div className="p4-stack">
      <section className="provider-form" aria-labelledby="workspace-heading">
        <h4 className="settings__label" id="workspace-heading">
          Workspace folder
        </h4>
        {dirLoading && (
          <p className="muted" role="status">
            <span className="spinner" aria-hidden="true" /> Reading workspace…
          </p>
        )}
        {dirError && !dirLoading && (
          <div className="notice notice--error" role="alert">
            <p className="notice__title">Couldn’t read the workspace folder.</p>
            <p className="notice__body">{dirError}</p>
            <p className="notice__body">
              <button type="button" className="btn" onClick={() => void loadDir()}>
                Retry
              </button>
            </p>
          </div>
        )}
        {!dirLoading && !dirError && (
          <p className="muted" role="status">
            {dir ? <code>{dir}</code> : "Not set — workspace tools stay disabled."}
          </p>
        )}
        <div className="provider-form__actions">
          <button
            type="button"
            className="btn"
            disabled={dirBusy}
            onClick={() => void doPick()}
          >
            {dirBusy ? "Working…" : "Pick…"}
          </button>
          <button
            type="button"
            className="btn"
            disabled={dirBusy || dir == null}
            onClick={() => void doClear()}
          >
            Clear
          </button>
        </div>
        <p className="settings__hint">
          Agents with the workspace permission can read and write under this
          folder. Leave it unset to keep them chat-only.
        </p>
      </section>

      <section className="provider-form" aria-labelledby="search-key-heading">
        <h4 className="settings__label" id="search-key-heading">
          Web-search key
        </h4>
        {keyLoading && (
          <p className="muted" role="status">
            <span className="spinner" aria-hidden="true" /> Reading key status…
          </p>
        )}
        {keyError && !keyLoading && (
          <div className="notice notice--error" role="alert">
            <p className="notice__title">Couldn’t read the key status.</p>
            <p className="notice__body">{keyError}</p>
            <p className="notice__body">
              <button type="button" className="btn" onClick={() => void loadKey()}>
                Retry
              </button>
            </p>
          </div>
        )}
        {!keyLoading && !keyError && (
          <p className="muted" role="status">
            {keySaved ? "Saved in the OS credential store." : "Not set — web search stays disabled."}
          </p>
        )}
        <div className="settings__field">
          <label className="settings__label" htmlFor="search-key-input">
            Tavily-compatible API key
          </label>
          <div className="settings__key-row">
            <input
              id="search-key-input"
              type="password"
              className="input"
              autoComplete="off"
              spellCheck={false}
              placeholder={keySaved ? "Enter a new key to replace it" : "tvly-…"}
              value={keyInput}
              onChange={(e) => setKeyInput(e.target.value)}
            />
            <button
              type="button"
              className="btn btn--primary"
              disabled={keyBusy || keyInput.trim() === ""}
              onClick={() => void doSaveKey()}
            >
              {keyBusy ? "Saving…" : "Save"}
            </button>
            <button
              type="button"
              className="btn"
              disabled={keyBusy || !keySaved}
              onClick={() => void doClearKey()}
            >
              Clear
            </button>
          </div>
        </div>
        <p className="settings__hint">
          Agents with the network permission use this key for web search. It
          lives only in the OS credential store — never in exports or logs.
        </p>
      </section>

      <section className="provider-form" aria-labelledby="mcp-heading">
        <h4 className="settings__label" id="mcp-heading">
          MCP servers
        </h4>
        {serversLoading && (
          <p className="muted" role="status">
            <span className="spinner" aria-hidden="true" /> Loading servers…
          </p>
        )}
        {serversError && (
          <div className="notice notice--error" role="alert">
            <p className="notice__title">Couldn’t load MCP servers.</p>
            <p className="notice__body">{serversError}</p>
            <p className="notice__body">
              <button type="button" className="btn" onClick={() => void loadServers()}>
                Retry
              </button>
            </p>
          </div>
        )}
        {!serversLoading && !serversError && servers.length === 0 && (
          <p className="muted">No MCP servers yet.</p>
        )}
        {!serversLoading && !serversError && servers.length > 0 && (
          <ul className="provider-list">
            {servers.map((server) => {
              const expanded = expandedId === server.id;
              const tools = toolsByServer[server.id] ?? [];
              return (
                <li key={server.id} className="provider-row p4-mcp-row">
                  <div className="provider-row__main">
                    <span className="provider-row__name">
                      {server.name}{" "}
                      <span className="room-badge">{server.transport}</span>{" "}
                      {!server.enabled && <span className="muted">(disabled)</span>}
                    </span>
                    <span className="provider-row__base">
                      {server.transport === "stdio"
                        ? [server.command ?? "", ...(server.args ?? [])]
                            .filter((p) => p !== "")
                            .join(" ") || "—"
                        : (server.url ?? "—")}
                    </span>
                    {expanded && (
                      <span className="p4-mcp-tools">
                        {toolsLoadingId === server.id && (
                          <span className="muted" role="status">
                            <span className="spinner" aria-hidden="true" /> Loading tools…
                          </span>
                        )}
                        {toolsError && toolsLoadingId == null && expanded && (
                          <span className="notice notice--error" role="alert">
                            <span className="notice__body">{toolsError} </span>
                            <button
                              type="button"
                              className="btn btn--sm"
                              onClick={() => void toggleTools(server)}
                            >
                              Retry
                            </button>
                          </span>
                        )}
                        {toolsLoadingId !== server.id && !toolsError && tools.length === 0 && (
                          <span className="muted">No tools reported.</span>
                        )}
                        {toolsLoadingId !== server.id &&
                          tools.map((t) => (
                            <span key={t.name} className="room-badge" title={t.description}>
                              {t.name}
                            </span>
                          ))}
                      </span>
                    )}
                  </div>
                  <div className="provider-row__actions">
                    <label className="check-row" title="Enable this server">
                      <input
                        type="checkbox"
                        aria-label={`Enable ${server.name}`}
                        checked={server.enabled}
                        disabled={rowBusy}
                        onChange={() => void doToggle(server)}
                      />
                    </label>
                    <button
                      type="button"
                      className="btn btn--sm"
                      onClick={() => void toggleTools(server)}
                    >
                      {expanded ? "Hide tools" : "View tools"}
                    </button>
                    {pendingDeleteId === server.id ? (
                      <button
                        type="button"
                        className="btn btn--danger btn--sm"
                        disabled={rowBusy}
                        onClick={() => void doRemove(server.id)}
                      >
                        Confirm?
                      </button>
                    ) : (
                      <button
                        type="button"
                        className="btn btn--sm"
                        onClick={() => {
                          setPendingDeleteId(server.id);
                          window.setTimeout(() => {
                            setPendingDeleteId((cur) =>
                              cur === server.id ? null : cur,
                            );
                          }, 4000);
                        }}
                      >
                        Remove
                      </button>
                    )}
                  </div>
                </li>
              );
            })}
          </ul>
        )}

        <h4 className="settings__label">Add server</h4>
        <label className="settings__label" htmlFor="mcp-name">
          Name
        </label>
        <input
          id="mcp-name"
          className="input"
          value={name}
          onChange={(e) => setName(e.target.value)}
          autoComplete="off"
          placeholder="e.g. filesystem"
        />
        <label className="settings__label" htmlFor="mcp-transport">
          Transport
        </label>
        <select
          id="mcp-transport"
          className="select"
          value={transport}
          onChange={(e) => setTransport(e.target.value as Transport)}
        >
          <option value="stdio">stdio</option>
          <option value="http">http</option>
        </select>
        {transport === "stdio" ? (
          <>
            <label className="settings__label" htmlFor="mcp-command">
              Command
            </label>
            <input
              id="mcp-command"
              className="input"
              value={command}
              onChange={(e) => setCommand(e.target.value)}
              autoComplete="off"
              spellCheck={false}
              placeholder="e.g. npx"
            />
            <label className="settings__label" htmlFor="mcp-args">
              Args (space-separated)
            </label>
            <input
              id="mcp-args"
              className="input"
              value={argsText}
              onChange={(e) => setArgsText(e.target.value)}
              autoComplete="off"
              spellCheck={false}
              placeholder="e.g. -y @modelcontextprotocol/server-filesystem /tmp"
            />
          </>
        ) : (
          <>
            <label className="settings__label" htmlFor="mcp-url">
              URL
            </label>
            <input
              id="mcp-url"
              className="input"
              type="url"
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              autoComplete="off"
              spellCheck={false}
              placeholder="https://example.com/mcp"
            />
          </>
        )}
        {addError && (
          <div className="notice notice--error" role="alert">
            <p className="notice__body">{addError}</p>
          </div>
        )}
        <div className="provider-form__actions">
          <button
            type="button"
            className="btn btn--primary"
            disabled={addBusy}
            onClick={() => void doAdd()}
          >
            {addBusy ? "Adding…" : "Add server"}
          </button>
        </div>
      </section>
    </div>
  );
}
