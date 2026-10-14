import { useCallback, useEffect, useState } from "react";
import { auditList, type AuditRow } from "../lib/api";
import { errorMessage } from "../lib/tauri";

interface AuditLogProps {
  /** null = global log (Settings). */
  roomId: string | null;
  agentName?: (id: string | null) => string;
  defaultOpen?: boolean;
}

function formatTime(ts: number): string {
  const ms = ts > 1e12 ? ts : ts > 1e9 ? ts * 1000 : ts;
  try {
    return new Date(ms).toLocaleString();
  } catch {
    return String(ts);
  }
}

function shortText(value: string, max: number): string {
  if (value.length <= max) return value;
  return `${value.slice(0, max)}…`;
}

export default function AuditLog({ roomId, agentName, defaultOpen }: AuditLogProps) {
  const [rows, setRows] = useState<AuditRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<number | null>(null);
  const [reloadKey, setReloadKey] = useState(0);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setRows(await auditList(roomId, 100));
    } catch (err: unknown) {
      setError(errorMessage(err));
    } finally {
      setLoading(false);
    }
  }, [roomId]);

  useEffect(() => {
    void load();
  }, [load, reloadKey]);

  const title = roomId == null ? "Audit log" : "Audit log";

  return (
    <details className="card p4-audit" open={defaultOpen ?? false}>
      <summary className="p4-audit__summary">
        {title}
        {!loading && !error && (
          <span className="muted"> · {rows.length} entries</span>
        )}
      </summary>
      <div className="p4-audit__body">
        {loading && (
          <p className="muted" role="status">
            <span className="spinner" aria-hidden="true" /> Loading audit…
          </p>
        )}
        {error && !loading && (
          <div className="notice notice--error" role="alert">
            <p className="notice__title">Couldn’t load the audit log.</p>
            <p className="notice__body">{error}</p>
            <p className="notice__body">
              <button
                type="button"
                className="btn"
                onClick={() => setReloadKey((k) => k + 1)}
              >
                Retry
              </button>
            </p>
          </div>
        )}
        {!loading && !error && rows.length === 0 && (
          <p className="muted">No audit entries yet.</p>
        )}
        {!loading && !error && rows.length > 0 && (
          <ul className="p4-audit__list">
            {rows.map((row) => {
              const open = expanded === row.id;
              return (
                <li key={row.id} className="p4-audit__row">
                  <button
                    type="button"
                    className="p4-audit__rowbtn"
                    aria-expanded={open}
                    onClick={() =>
                      setExpanded((cur) => (cur === row.id ? null : row.id))
                    }
                  >
                    <span className="p4-audit__time">{formatTime(row.created_at)}</span>
                    <span className="p4-audit__agent">
                      {agentName ? agentName(row.agent_id) : (row.agent_id ?? "—")}
                    </span>
                    <span className="p4-audit__tool">
                      <code>{shortText(row.tool, 32)}</code>
                    </span>
                    <span className={`p4-audit__decision p4-audit__decision--${row.decision}`}>
                      {row.decision}
                    </span>
                  </button>
                  {open && (
                    <div className="p4-audit__detail">
                      <span className="settings__label">Arguments</span>
                      <pre className="room-tool__args">{row.args_json}</pre>
                      {row.result_summary !== "" && (
                        <>
                          <span className="settings__label">Result</span>
                          <p className="msg__text">{row.result_summary}</p>
                        </>
                      )}
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
        )}
        {!loading && !error && rows.length > 0 && (
          <p>
            <button
              type="button"
              className="btn btn--sm"
              onClick={() => setReloadKey((k) => k + 1)}
            >
              Refresh
            </button>
          </p>
        )}
      </div>
    </details>
  );
}
