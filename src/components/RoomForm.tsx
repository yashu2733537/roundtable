import { useState } from "react";
import type { FormEvent } from "react";
import { AgentAvatar } from "./AgentsPage";
import { MemberPermissionsRow } from "./AgentAccess";
import { useIsAndroid } from "./useAndroid";
import type { Agent, RoomDetail, RoomMode, ToolProtocol } from "../lib/api";

export interface RoomFormValues {
  title: string;
  mode: RoomMode;
  /** Ordered member agent ids; seat = index. */
  memberIds: string[];
  leadAgentId: string | null;
  maxTurns: number;
  tokenBudget: number;
  timeoutSec: number;
  loopDetection: boolean;
  /** agent id → allowed targets ("room" = whole-room broadcast). */
  allowlist: Record<string, string[]>;
  /** agent id → tool wire protocol. */
  protocols: Record<string, ToolProtocol>;
}

interface RoomFormProps {
  agents: Agent[];
  initial: RoomFormValues;
  heading: string;
  submitLabel: string;
  saving: boolean;
  serverError: string | null;
  onSubmit: (values: RoomFormValues) => void;
  onCancel: () => void;
}

const MODE_HINTS: Record<RoomMode, string> = {
  directed: "@mention one agent, only it replies",
  orchestrated: "lead plans, delegates to workers, synthesizes — build first, default",
  round_robin: "agents take turns until someone calls finish — opt-in",
};

const PROTOCOLS: ToolProtocol[] = ["auto", "native", "json", "text"];

export function blankRoomFormValues(): RoomFormValues {
  return {
    title: "",
    mode: "orchestrated",
    memberIds: [],
    leadAgentId: null,
    maxTurns: 20,
    tokenBudget: 20000,
    timeoutSec: 120,
    loopDetection: true,
    allowlist: {},
    protocols: {},
  };
}

export function roomFormValuesFromDetail(
  detail: RoomDetail,
  agentsById: Record<string, Agent>,
): RoomFormValues {
  const ordered = [...detail.members].sort((a, b) => a.seat - b.seat);
  const allowlist: Record<string, string[]> = {};
  for (const m of ordered) {
    allowlist[m.agent_id] = [...m.can_talk_to];
  }
  const protocols: Record<string, ToolProtocol> = {};
  for (const m of ordered) {
    protocols[m.agent_id] = agentsById[m.agent_id]?.tool_protocol ?? "auto";
  }
  return {
    title: detail.room.title,
    mode: detail.room.mode,
    memberIds: ordered.map((m) => m.agent_id),
    leadAgentId: detail.room.lead_agent_id,
    maxTurns: detail.room.max_turns,
    tokenBudget: detail.room.token_budget,
    timeoutSec: Math.max(1, Math.round(detail.room.timeout_ms / 1000)),
    loopDetection: detail.room.loop_detection,
    allowlist,
    protocols,
  };
}

function toInt(raw: string, fallback: number): number {
  const n = Number(raw);
  if (!Number.isFinite(n)) return fallback;
  return Math.max(0, Math.floor(n));
}

export default function RoomForm({
  agents,
  initial,
  heading,
  submitLabel,
  saving,
  serverError,
  onSubmit,
  onCancel,
}: RoomFormProps) {
  const [title, setTitle] = useState(initial.title);
  const [mode, setMode] = useState<RoomMode>(initial.mode);
  const [checked, setChecked] = useState<string[]>(initial.memberIds);
  const [lead, setLead] = useState<string>(initial.leadAgentId ?? "");
  const [maxTurns, setMaxTurns] = useState(initial.maxTurns);
  const [tokenBudget, setTokenBudget] = useState(initial.tokenBudget);
  const [timeoutSec, setTimeoutSec] = useState(initial.timeoutSec);
  const [loop, setLoop] = useState(initial.loopDetection);
  const [allow, setAllow] = useState<Record<string, string[]>>(initial.allowlist);
  const [proto, setProto] = useState<Record<string, ToolProtocol>>(initial.protocols);
  const [localError, setLocalError] = useState<string | null>(null);
  const isAndroid = useIsAndroid();

  const agentsById: Record<string, Agent> = {};
  for (const a of agents) agentsById[a.id] = a;

  const toggleMember = (id: string) => {
    setChecked((prev) => {
      if (prev.includes(id)) {
        return prev.filter((m) => m !== id);
      }
      return [...prev, id];
    });
    setAllow((prev) => {
      if (prev[id]) return prev;
      return { ...prev, [id]: ["room"] };
    });
  };

  const toggleAllowTarget = (memberId: string, target: string) => {
    setAllow((prev) => {
      const current = prev[memberId] ?? ["room"];
      const next = current.includes(target)
        ? current.filter((t) => t !== target)
        : [...current, target];
      return { ...prev, [memberId]: next };
    });
  };

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (title.trim() === "") {
      setLocalError("Give the room a title.");
      return;
    }
    if (checked.length === 0) {
      setLocalError("Pick at least one enabled agent as a member.");
      return;
    }
    if (mode === "orchestrated" && (lead === "" || !checked.includes(lead))) {
      setLocalError("Orchestrated rooms need a lead agent that is also a member.");
      return;
    }
    if (maxTurns < 1 || tokenBudget < 1 || timeoutSec < 1) {
      setLocalError("Limits must each be at least 1.");
      return;
    }
    setLocalError(null);
    const allowlist: Record<string, string[]> = {};
    for (const id of checked) {
      allowlist[id] = allow[id] ?? ["room"];
    }
    const protocols: Record<string, ToolProtocol> = {};
    for (const id of checked) {
      protocols[id] = proto[id] ?? agentsById[id]?.tool_protocol ?? "auto";
    }
    onSubmit({
      title: title.trim(),
      mode,
      memberIds: checked,
      leadAgentId: mode === "orchestrated" ? lead : null,
      maxTurns,
      tokenBudget,
      timeoutSec,
      loopDetection: loop,
      allowlist,
      protocols,
    });
  };

  return (
    <section className="card builder" aria-labelledby="room-form-heading">
      <h3 className="card__title" id="room-form-heading">
        {heading}
      </h3>
      <form onSubmit={submit}>
        <div className="builder__grid">
          <div className="settings__field">
            <label className="settings__label" htmlFor="room-title">
              Title
            </label>
            <input
              id="room-title"
              className="input"
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              autoComplete="off"
              placeholder="e.g. Launch review crew"
            />
          </div>
          <div className="settings__field">
            <label className="settings__label" htmlFor="room-mode">
              Mode
            </label>
            <select
              id="room-mode"
              className="select"
              value={mode}
              onChange={(e) => setMode(e.target.value as RoomMode)}
            >
              <option value="orchestrated">Orchestrated</option>
              <option value="directed">Directed</option>
              <option value="round_robin">Round robin</option>
            </select>
            <p className="settings__hint">{MODE_HINTS[mode]}</p>
          </div>
        </div>

        {mode === "orchestrated" && (
          <div className="settings__field">
            <label className="settings__label" htmlFor="room-lead">
              Lead agent (must be a member)
            </label>
            <select
              id="room-lead"
              className="select"
              value={lead}
              onChange={(e) => setLead(e.target.value)}
            >
              <option value="">Pick a lead…</option>
              {checked.map((id) => (
                <option key={id} value={id}>
                  {agentsById[id]?.name ?? id}
                </option>
              ))}
            </select>
          </div>
        )}

        <div className="settings__field">
          <span className="settings__label" id="room-members-label">
            Members (seat order = the order you check them)
          </span>
          <ul className="room-form__members" aria-labelledby="room-members-label">
            {agents.map((agent) => (
              <li key={agent.id}>
                <label className={`check-row${agent.enabled ? "" : " check-row--disabled"}`}>
                  <input
                    type="checkbox"
                    checked={checked.includes(agent.id)}
                    disabled={!agent.enabled}
                    onChange={() => toggleMember(agent.id)}
                  />
                  <AgentAvatar agent={agent} size={20} /> {agent.name}
                  {!agent.enabled && <span className="muted">(disabled)</span>}
                </label>
              </li>
            ))}
          </ul>
          {agents.length === 0 && (
            <p className="settings__hint">No agents yet — create one on the Agents page first.</p>
          )}
        </div>

        <div className="builder__grid">
          <div className="settings__field">
            <label className="settings__label" htmlFor="room-turns">
              Max turns
            </label>
            <input
              id="room-turns"
              type="number"
              className="input"
              min={1}
              value={maxTurns}
              onChange={(e) => setMaxTurns(toInt(e.target.value, maxTurns))}
            />
          </div>
          <div className="settings__field">
            <label className="settings__label" htmlFor="room-budget">
              Token budget
            </label>
            <input
              id="room-budget"
              type="number"
              className="input"
              min={1}
              value={tokenBudget}
              onChange={(e) => setTokenBudget(toInt(e.target.value, tokenBudget))}
            />
          </div>
          <div className="settings__field">
            <label className="settings__label" htmlFor="room-timeout">
              Timeout (seconds)
            </label>
            <input
              id="room-timeout"
              type="number"
              className="input"
              min={1}
              value={timeoutSec}
              onChange={(e) => setTimeoutSec(toInt(e.target.value, timeoutSec))}
            />
          </div>
          <div className="settings__field">
            <label className="check-row" htmlFor="room-loop">
              <input
                id="room-loop"
                type="checkbox"
                checked={loop}
                onChange={(e) => setLoop(e.target.checked)}
              />
              Loop detection
            </label>
          </div>
        </div>

        {checked.length > 0 && (
          <div className="settings__field">
            <span className="settings__label">Who each member may message</span>
            <ul className="room-form__allow">
              {checked.map((id) => {
                const agent = agentsById[id];
                if (!agent) return null;
                const targets = allow[id] ?? ["room"];
                const others = checked.filter((o) => o !== id);
                return (
                  <li key={id} className="room-allow__row">
                    <span className="room-allow__name">
                      <AgentAvatar agent={agent} size={20} /> {agent.name}
                    </span>
                    <span className="room-allow__checks">
                      <label className="check-row">
                        <input
                          type="checkbox"
                          checked={targets.includes("room")}
                          onChange={() => toggleAllowTarget(id, "room")}
                        />
                        whole room
                      </label>
                      {others.map((other) => (
                        <label className="check-row" key={other}>
                          <input
                            type="checkbox"
                            checked={targets.includes(other)}
                            onChange={() => toggleAllowTarget(id, other)}
                          />
                          {agentsById[other]?.name ?? other}
                        </label>
                      ))}
                    </span>
                    {!isAndroid && <MemberPermissionsRow agent={agent} />}
                  </li>
                );
              })}
            </ul>
          </div>
        )}

        {checked.length > 0 && (
          <div className="settings__field">
            <span className="settings__label">Tool protocol per member</span>
            <ul className="room-form__allow">
              {checked.map((id) => {
                const agent = agentsById[id];
                if (!agent) return null;
                return (
                  <li key={id} className="room-allow__row">
                    <span className="room-allow__name">
                      <AgentAvatar agent={agent} size={20} /> {agent.name}
                    </span>
                    <select
                      className="select room-allow__select"
                      aria-label={`Tool protocol for ${agent.name}`}
                      value={proto[id] ?? agent.tool_protocol ?? "auto"}
                      onChange={(e) =>
                        setProto((prev) => ({ ...prev, [id]: e.target.value as ToolProtocol }))
                      }
                    >
                      {PROTOCOLS.map((p) => (
                        <option key={p} value={p}>
                          {p}
                        </option>
                      ))}
                    </select>
                  </li>
                );
              })}
            </ul>
          </div>
        )}

        {localError && (
          <div className="notice notice--error" role="alert">
            <p className="notice__body">{localError}</p>
          </div>
        )}
        {serverError && (
          <div className="notice notice--error" role="alert">
            <p className="notice__body">{serverError}</p>
          </div>
        )}

        <div className="provider-form__actions">
          <button type="submit" className="btn btn--primary" disabled={saving}>
            {saving ? "Saving…" : submitLabel}
          </button>
          <button type="button" className="btn" onClick={onCancel}>
            Cancel
          </button>
        </div>
      </form>
    </section>
  );
}
