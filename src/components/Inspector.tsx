import type { RefObject } from "react";
import EmptyState from "./EmptyState";
import { IconSliders } from "./Icon";
import { parseRunMessages, type Agent, type ProviderInfo, type RunInfo } from "../lib/api";
import type { AppInfoState } from "../lib/types";

export type RunInfoState =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "ready"; run: RunInfo | null }
  | { status: "error"; message: string };

interface InspectorProps {
  paneRef: RefObject<HTMLElement>;
  onOpenSettings: () => void;
  appInfo: AppInfoState;
  conversationTitle: string | null;
  run: RunInfoState;
  agents: Agent[];
  providers: ProviderInfo[];
}

function formatLatency(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  return `${(ms / 1000).toFixed(1)} s`;
}

function formatTokens(value: number | null): string {
  if (value == null) return "—";
  return value.toLocaleString();
}

export default function Inspector({
  paneRef,
  onOpenSettings,
  appInfo,
  conversationTitle,
  run,
  agents,
  providers,
}: InspectorProps) {
  const runInfo = run.status === "ready" ? run.run : null;
  const runMessages = parseRunMessages(runInfo);
  const agentName = runInfo?.agent_id
    ? (agents.find((a) => a.id === runInfo.agent_id)?.name ?? "Removed agent")
    : "—";
  const providerName = runInfo?.provider_id
    ? runInfo.provider_id === "ollama"
      ? "Ollama (local)"
      : (providers.find((p) => p.id === runInfo.provider_id)?.name ?? runInfo.provider_id)
    : "—";

  return (
    <aside
      className="pane pane--inspector"
      ref={paneRef}
      tabIndex={-1}
      aria-label="Inspector"
    >
      <header className="pane__header">
        <h2 className="pane__title">Inspector</h2>
        <div className="pane__header-actions">
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
      <div className="pane__body">
        <section className="card" aria-labelledby="about-heading">
          <h3 className="card__title" id="about-heading">
            About
          </h3>
          {appInfo.status === "loading" && (
            <p className="muted" role="status">
              <span className="spinner" aria-hidden="true" /> Loading app
              info…
            </p>
          )}
          {appInfo.status === "error" && (
            <div className="notice notice--error" role="alert">
              <p className="notice__title">Couldn’t read app info.</p>
              <p className="notice__body">
                {appInfo.message} If you ran <code>npm run dev</code> in a
                browser instead of <code>npm run tauri dev</code>, Rust
                commands are unavailable — restart with{" "}
                <code>npm run tauri dev</code>.
              </p>
            </div>
          )}
          {appInfo.status === "ready" && (
            <dl className="kv">
              <div className="kv__row">
                <dt>App</dt>
                <dd>{appInfo.info.name}</dd>
              </div>
              <div className="kv__row">
                <dt>Version</dt>
                <dd>{appInfo.info.version}</dd>
              </div>
              <div className="kv__row">
                <dt>Platform</dt>
                <dd>{appInfo.info.platform}</dd>
              </div>
            </dl>
          )}
        </section>

        {conversationTitle == null ? (
          <div className="pane__body--flush">
            <EmptyState
              title="Nothing selected"
              description="Details about the selected conversation, its messages, and its agents will appear here."
            />
          </div>
        ) : (
          <section className="card" aria-labelledby="run-heading">
            <h3 className="card__title" id="run-heading">
              Last run · {conversationTitle}
            </h3>
            {run.status === "loading" && (
              <p className="muted" role="status">
                <span className="spinner" aria-hidden="true" /> Loading run…
              </p>
            )}
            {run.status === "error" && (
              <div className="notice notice--error" role="alert">
                <p className="notice__title">Couldn’t load the run.</p>
                <p className="notice__body">{run.message}</p>
              </div>
            )}
            {run.status === "ready" && runInfo == null && (
              <p className="muted">
                No model call yet — send a message to record one.
              </p>
            )}
            {run.status === "ready" && runInfo != null && (
              <>
                <dl className="kv">
                  <div className="kv__row">
                    <dt>Model</dt>
                    <dd>{runInfo.model || "—"}</dd>
                  </div>
                  <div className="kv__row">
                    <dt>Agent</dt>
                    <dd>{agentName}</dd>
                  </div>
                  <div className="kv__row">
                    <dt>Provider</dt>
                    <dd>{providerName}</dd>
                  </div>
                  <div className="kv__row">
                    <dt>Latency</dt>
                    <dd>{formatLatency(runInfo.latency_ms)}</dd>
                  </div>
                  <div className="kv__row">
                    <dt>Prompt tokens</dt>
                    <dd>{formatTokens(runInfo.prompt_tokens)}</dd>
                  </div>
                  <div className="kv__row">
                    <dt>Completion tokens</dt>
                    <dd>{formatTokens(runInfo.completion_tokens)}</dd>
                  </div>
                  <div className="kv__row">
                    <dt>Total tokens</dt>
                    <dd>{formatTokens(runInfo.total_tokens)}</dd>
                  </div>
                </dl>
                <p className="settings__hint">
                  Token counts come from the provider when it reports usage;
                  otherwise they show —.
                </p>
                <details className="run-messages">
                  <summary>
                    Messages sent to the model ({runMessages.length})
                  </summary>
                  <ol className="run-messages__list">
                    {runMessages.map((message, index) => (
                      <li key={index} className="run-messages__item">
                        <span className="msg__role">{message.role}</span>
                        <p className="msg__text">{message.content}</p>
                      </li>
                    ))}
                  </ol>
                </details>
              </>
            )}
          </section>
        )}
      </div>
    </aside>
  );
}
