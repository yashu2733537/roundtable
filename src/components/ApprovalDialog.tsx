import { useState } from "react";
import Modal from "./Modal";
import { errorMessage } from "../lib/tauri";
import { approveCall, type ApprovalDecision } from "../lib/api";

export interface PendingApproval {
  call_id: string;
  agent_id: string;
  tool: string;
  summary: string;
  command: string | null;
}

interface ApprovalDialogProps {
  approval: PendingApproval;
  agentName: string;
  queueLength: number;
  onResolved: () => void;
}

/**
 * Approval modal. It deliberately has no silent-dismiss path: Esc and the
 * backdrop are swallowed so the run stays parked until the user picks
 * Allow once / Always / Deny explicitly.
 */
export default function ApprovalDialog({
  approval,
  agentName,
  queueLength,
  onResolved,
}: ApprovalDialogProps) {
  const [busy, setBusy] = useState<ApprovalDecision | null>(null);
  const [error, setError] = useState<string | null>(null);

  const decide = async (decision: ApprovalDecision) => {
    setBusy(decision);
    setError(null);
    try {
      await approveCall(approval.call_id, decision);
      onResolved();
    } catch (err: unknown) {
      setError(errorMessage(err));
    } finally {
      setBusy(null);
    }
  };

  return (
    <Modal open label="Tool approval" onClose={() => undefined} className="p4-approval">
      <div className="modal__header">
        <h2 className="modal__title">Tool approval needed</h2>
      </div>
      <div className="p4-approval__body">
        <p className="p4-approval__lead">
          <strong>{agentName}</strong> wants to run <strong>{approval.tool}</strong>
          {queueLength > 1 ? ` (${queueLength} pending)` : ""}.
        </p>
        <p className="msg__text">{approval.summary}</p>
        {approval.command != null && approval.command !== "" && (
          <div className="p4-approval__cmd">
            <span className="settings__label">Exact command</span>
            <pre className="room-tool__args">
              <code>{approval.command}</code>
            </pre>
          </div>
        )}
        <p className="settings__hint">
          The run is paused until you choose. Deny explicitly to stop the tool
          call — closing this dialog never approves or denies it.
        </p>
        {error && (
          <div className="notice notice--error" role="alert">
            <p className="notice__title">Couldn’t send your decision.</p>
            <p className="notice__body">{error}</p>
            <p className="notice__body">
              <button
                type="button"
                className="btn"
                disabled={busy != null}
                onClick={() => {
                  setError(null);
                }}
              >
                Retry
              </button>
            </p>
          </div>
        )}
        <div className="provider-form__actions p4-approval__actions">
          <button
            type="button"
            className="btn btn--primary"
            disabled={busy != null}
            onClick={() => void decide("once")}
            autoFocus
          >
            {busy === "once" ? "Allowing…" : "Allow once"}
          </button>
          <button
            type="button"
            className="btn"
            disabled={busy != null}
            onClick={() => void decide("always")}
          >
            {busy === "always" ? "Saving…" : "Always for this agent"}
          </button>
          <button
            type="button"
            className="btn btn--danger"
            disabled={busy != null}
            onClick={() => void decide("deny")}
          >
            {busy === "deny" ? "Denying…" : "Deny"}
          </button>
        </div>
      </div>
    </Modal>
  );
}
