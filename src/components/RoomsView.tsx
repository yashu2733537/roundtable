import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { FormEvent, RefObject } from "react";
import EmptyState from "./EmptyState";
import { AgentAvatar } from "./AgentsPage";
import ApprovalDialog, { type PendingApproval } from "./ApprovalDialog";
import AuditLog from "./AuditLog";
import { IconCommand, IconPanelDetail } from "./Icon";
import RoomGraph from "./RoomGraph";
import { spacerHeights, useVirtualWindow } from "./useVirtualList";
import RoomForm, {
  blankRoomFormValues,
  roomFormValuesFromDetail,
  type RoomFormValues,
} from "./RoomForm";
import {
  createRoom,
  deleteRoom,
  errorMessage,
  fetchRoomRunState,
  getRoom,
  listRoomMessages,
  listRooms,
  newStreamId,
  pauseRoom,
  resumeRoom,
  searchAgents,
  setAgentProtocol,
  setRoomMembers,
  startRoom,
  stepRoom,
  stopRoom,
  updateRoom,
  type Agent,
  type Room,
  type RoomDetail,
  type RoomEvent,
  type RoomMessage,
  type RoomRunState,
} from "../lib/api";

interface RoomsViewProps {
  paneRef: RefObject<HTMLElement>;
  onOpenPalette: () => void;
  onBack: () => void;
  onOpenInspector?: () => void;
}

interface MentionState {
  query: string;
  start: number;
}

type FormState = { mode: "create" } | { mode: "edit" };

const STOP_REASON_TEXT: Record<string, string> = {
  turn_limit: "Turn limit reached.",
  token_budget: "Token budget exceeded.",
  timeout: "Time budget exceeded.",
  loop_detected: "Stopped: two agents repeated the same exchange — looks like a loop.",
  stopped_by_user: "Stopped by you.",
  no_agent_mentioned: "Mention an agent with @Name to direct a reply.",
  reply_complete: "Replied.",
  lead_finished: "Lead synthesized the final answer.",
  finished_by_agent: "An agent called finish.",
};

function stopReasonText(state: RoomRunState): string | null {
  if (state.stop_reason) {
    return STOP_REASON_TEXT[state.stop_reason] ?? state.stop_reason;
  }
  if (state.status === "done") return "Done.";
  return null;
}

function modeLabel(mode: Room["mode"]): string {
  if (mode === "directed") return "Directed";
  if (mode === "round_robin") return "Round robin";
  return "Orchestrated";
}

export default function RoomsView({ paneRef, onOpenPalette, onBack, onOpenInspector }: RoomsViewProps) {
  const [agents, setAgents] = useState<Agent[]>([]);
  const [agentsLoading, setAgentsLoading] = useState(true);
  const [agentsError, setAgentsError] = useState<string | null>(null);

  const [rooms, setRooms] = useState<Room[]>([]);
  const [roomsLoading, setRoomsLoading] = useState(true);
  const [roomsError, setRoomsError] = useState<string | null>(null);
  const [runStates, setRunStates] = useState<Record<string, RoomRunState>>({});
  const [membersByRoom, setMembersByRoom] = useState<Record<string, string[]>>({});
  const [roomSearch, setRoomSearch] = useState("");
  const [pendingDeleteId, setPendingDeleteId] = useState<string | null>(null);
  const [rowBusy, setRowBusy] = useState(false);

  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<RoomDetail | null>(null);
  const [messages, setMessages] = useState<RoomMessage[]>([]);
  const [runState, setRunState] = useState<RoomRunState | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [detailError, setDetailError] = useState<string | null>(null);

  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [activeStream, setActiveStream] = useState<string | null>(null);
  const [streamError, setStreamError] = useState<string | null>(null);
  const [composer, setComposer] = useState("");
  const [approvals, setApprovals] = useState<PendingApproval[]>([]);
  const [mention, setMention] = useState<MentionState | null>(null);
  const [mentionIndex, setMentionIndex] = useState(0);
  const [directedHint, setDirectedHint] = useState(false);

  const [form, setForm] = useState<FormState | null>(null);
  const [formKey, setFormKey] = useState(0);
  const [formSaving, setFormSaving] = useState(false);
  const [formServerError, setFormServerError] = useState<string | null>(null);

  const scrollRef = useRef<HTMLDivElement>(null);
  const stickRef = useRef(true);
  const composerRef = useRef<HTMLTextAreaElement>(null);
  const selectedIdRef = useRef<string | null>(null);

  // Virtualized room log: full list under the threshold, slice + spacers above it.
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

  const agentsById = useMemo(() => {
    const map: Record<string, Agent> = {};
    for (const a of agents) map[a.id] = a;
    return map;
  }, [agents]);

  const memberAgents = useMemo(() => {
    if (!detail) return [];
    return [...detail.members]
      .sort((a, b) => a.seat - b.seat)
      .map((m) => agentsById[m.agent_id])
      .filter((a): a is Agent => a != null);
  }, [detail, agentsById]);

  const mentionable = useMemo(
    () => memberAgents.filter((a) => a.enabled),
    [memberAgents],
  );

  const mentionMatches = useMemo(() => {
    if (!mention) return [];
    const query = mention.query.toLowerCase();
    return mentionable.filter((a) => a.name.toLowerCase().includes(query)).slice(0, 6);
  }, [mention, mentionable]);

  useEffect(() => {
    setMentionIndex(0);
  }, [mention?.query]);

  // Stick to the bottom while tokens arrive, unless the user scrolled up.
  useEffect(() => {
    const el = scrollRef.current;
    if (el && stickRef.current) {
      el.scrollTop = el.scrollHeight;
    }
  }, [messages, drafts, selectedId]);

  const refreshAgents = useCallback(async () => {
    setAgentsLoading(true);
    try {
      setAgents(await searchAgents("", undefined));
      setAgentsError(null);
    } catch (err: unknown) {
      setAgentsError(errorMessage(err));
    } finally {
      setAgentsLoading(false);
    }
  }, []);

  const refreshRooms = useCallback(async () => {
    setRoomsLoading(true);
    try {
      setRooms(await listRooms());
      setRoomsError(null);
    } catch (err: unknown) {
      setRoomsError(errorMessage(err));
    } finally {
      setRoomsLoading(false);
    }
  }, []);

  useEffect(() => {
    void refreshAgents();
    void refreshRooms();
  }, [refreshAgents, refreshRooms]);

  // Keep a run-status snapshot plus member ids per room so the list can show them.
  useEffect(() => {
    if (rooms.length === 0) return;
    let cancelled = false;
    void Promise.allSettled(
      rooms.map((room) =>
        Promise.all([
          fetchRoomRunState(room.id).then((state) => ({ id: room.id, state })),
          getRoom(room.id).then((found) => ({
            id: room.id,
            members: [...(found?.members ?? [])]
              .sort((a, b) => a.seat - b.seat)
              .map((m) => m.agent_id),
          })),
        ]),
      ),
    ).then((results) => {
      if (cancelled) return;
      setRunStates((prev) => {
        const next = { ...prev };
        for (const result of results) {
          if (result.status === "fulfilled") {
            next[result.value[0].id] = result.value[0].state;
          }
        }
        return next;
      });
      setMembersByRoom((prev) => {
        const next = { ...prev };
        for (const result of results) {
          if (result.status === "fulfilled") {
            next[result.value[1].id] = result.value[1].members;
          }
        }
        return next;
      });
    });
    return () => {
      cancelled = true;
    };
  }, [rooms]);

  const handleRoomEvent = useCallback(
    (roomId: string) => (event: RoomEvent) => {
      if (selectedIdRef.current !== roomId) return;
      if (event.type === "token") {
        setDrafts((prev) => ({
          ...prev,
          [event.agent_id]: (prev[event.agent_id] ?? "") + event.delta,
        }));
      } else if (event.type === "message") {
        const msg = event.message;
        setMessages((prev) => {
          // Drop the single oldest optimistic user row matching this echo.
          // (Older code dropped every temp with the same text, so sending
          // the same message twice flickered.)
          let dropped = false;
          const withoutTemps =
            msg.sender === "user"
              ? prev.filter((m) => {
                  if (
                    !dropped &&
                    m.id.startsWith("temp-") &&
                    m.content === msg.content
                  ) {
                    dropped = true;
                    return false;
                  }
                  return true;
                })
              : prev;
          if (withoutTemps.some((m) => m.id === msg.id)) {
            return withoutTemps.map((m) => (m.id === msg.id ? msg : m));
          }
          return [...withoutTemps, msg];
        });
        if (msg.sender_agent_id) {
          const sid = msg.sender_agent_id;
          setDrafts((prev) => {
            if (!prev[sid]) return prev;
            const next = { ...prev };
            delete next[sid];
            return next;
          });
        }
      } else if (event.type === "tool_call") {
        setMessages((prev) => {
          let target = -1;
          for (let i = prev.length - 1; i >= 0; i -= 1) {
            if (prev[i].sender_agent_id === event.agent_id) {
              target = i;
              break;
            }
          }
          if (target < 0) return prev;
          const row = prev[target];
          // The backend emits the requesting message first with pending
          // (id-less) calls, then live calls with real ids. Replace a
          // matching pending entry instead of appending a duplicate.
          const pendingIdx = row.tool_calls.findIndex(
            (c) =>
              c.id === event.call.id ||
              (c.id === "" && c.tool === event.call.tool),
          );
          if (pendingIdx >= 0 && row.tool_calls[pendingIdx].id === event.call.id) {
            return prev;
          }
          const nextCalls = [...row.tool_calls];
          if (pendingIdx >= 0) {
            nextCalls[pendingIdx] = event.call;
          } else {
            nextCalls.push(event.call);
          }
          const next = [...prev];
          next[target] = { ...row, tool_calls: nextCalls };
          return next;
        });
      } else if (event.type === "tool_result") {
        setMessages((prev) =>
          prev.map((m) => ({
            ...m,
            tool_calls: m.tool_calls.map((c) =>
              c.id === event.call_id ? { ...c, result: event.result } : c,
            ),
          })),
        );
      } else if (event.type === "run_state") {
        setRunState(event.state);
        setRunStates((prev) => ({ ...prev, [roomId]: event.state }));
        if (event.state.status !== "running") {
          setActiveStream(null);
        }
        if (event.state.status === "done" || event.state.status === "stopped") {
          listRoomMessages(roomId)
            .then((rows) => {
              if (selectedIdRef.current === roomId) setMessages(rows);
            })
            .catch(() => undefined);
        }
      } else if (event.type === "approval") {
        setApprovals((prev) =>
          prev.some((a) => a.call_id === event.call_id)
            ? prev
            : [
                ...prev,
                {
                  call_id: event.call_id,
                  agent_id: event.agent_id,
                  tool: event.tool,
                  summary: event.summary,
                  command: event.command,
                },
              ],
        );
      } else {
        setStreamError(event.message);
      }
    },
    [],
  );

  const selectRoom = useCallback(
    async (id: string) => {
      selectedIdRef.current = id;
      setSelectedId(id);
      setDetail(null);
      setMessages([]);
      setRunState(null);
      setDrafts({});
      setApprovals([]);
      setComposer("");
      setMention(null);
      setDirectedHint(false);
      setStreamError(null);
      setDetailError(null);
      setDetailLoading(true);
      setActiveStream(null);
      setForm(null);
      try {
        const [found, rows, state] = await Promise.all([
          getRoom(id),
          listRoomMessages(id),
          fetchRoomRunState(id),
        ]);
        if (selectedIdRef.current !== id) return;
        if (found == null) {
          setDetailError("This room no longer exists.");
          return;
        }
        setDetail(found);
        setMessages(rows);
        setRunState(state);
        setRunStates((prev) => ({ ...prev, [id]: state }));
        setMembersByRoom((prev) => ({
          ...prev,
          [id]: [...found.members].sort((a, b) => a.seat - b.seat).map((m) => m.agent_id),
        }));
      } catch (err: unknown) {
        if (selectedIdRef.current !== id) return;
        setDetailError(errorMessage(err));
      } finally {
        if (selectedIdRef.current === id) setDetailLoading(false);
      }
    },
    [],
  );

  const retryDetail = useCallback(() => {
    if (selectedId) void selectRoom(selectedId);
  }, [selectedId, selectRoom]);

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
    setComposer(`${before}@${agent.name} ${after}`);
    setMention(null);
    requestAnimationFrame(() => {
      const el = composerRef.current;
      if (!el) return;
      const pos = before.length + agent.name.length + 2;
      el.focus();
      el.setSelectionRange(pos, pos);
    });
  };

  /** First @Name in the text that matches an enabled member, if any. */
  const mentionAgentId = (text: string): string | null => {
    const pattern = /@([\w-]+)/g;
    let match: RegExpExecArray | null;
    while ((match = pattern.exec(text)) !== null) {
      const name = match[1].toLowerCase();
      const agent = mentionable.find((a) => a.name.toLowerCase() === name);
      if (agent) return agent.id;
    }
    return null;
  };

  const running = runState?.status === "running";
  const busy = running || activeStream != null;

  const send = useCallback(async () => {
    if (!detail || selectedIdRef.current !== detail.room.id) return;
    if (runState?.status === "running" || activeStream != null) return;
    const text = composer.trim();
    if (text === "") return;
    if (detail.room.mode === "directed" && mentionAgentId(text) == null) {
      setDirectedHint(true);
      return;
    }
    setDirectedHint(false);
    setComposer("");
    setMention(null);
    setStreamError(null);
    const sid = newStreamId();
    setActiveStream(sid);
    const optimistic: RoomMessage = {
      id: `temp-${sid}`,
      room_id: detail.room.id,
      sender: "user",
      sender_agent_id: null,
      recipient: "room",
      recipient_agent_id: null,
      content: text,
      tool_calls: [],
      model: "",
      prompt_tokens: null,
      completion_tokens: null,
      total_tokens: null,
      latency_ms: null,
      created_at: Date.now(),
    };
    setMessages((prev) => [...prev, optimistic]);
    try {
      await startRoom(detail.room.id, text, sid, handleRoomEvent(detail.room.id));
    } catch (err: unknown) {
      setActiveStream(null);
      setStreamError(errorMessage(err));
      try {
        const rows = await listRoomMessages(detail.room.id);
        if (selectedIdRef.current === detail.room.id) setMessages(rows);
      } catch {
        setMessages((prev) => prev.filter((m) => m.id !== optimistic.id));
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [detail, runState, activeStream, composer, mentionable, handleRoomEvent]);

  const onSendSubmit = (e: FormEvent) => {
    e.preventDefault();
    if (mention && mentionMatches.length > 0) {
      acceptMention(mentionMatches[mentionIndex] ?? mentionMatches[0]);
      return;
    }
    void send();
  };

  const doPause = async () => {
    if (!detail) return;
    try {
      const state = await pauseRoom(detail.room.id);
      setRunState(state);
      setRunStates((prev) => ({ ...prev, [detail.room.id]: state }));
    } catch (err: unknown) {
      setStreamError(errorMessage(err));
    }
  };

  const doResume = async () => {
    if (!detail) return;
    setStreamError(null);
    try {
      const state = await resumeRoom(detail.room.id);
      setRunState(state);
      setRunStates((prev) => ({ ...prev, [detail.room.id]: state }));
    } catch (err: unknown) {
      setStreamError(errorMessage(err));
    }
  };

  const doStep = async () => {
    if (!detail || runState?.status === "running" || activeStream != null) return;
    setStreamError(null);
    try {
      if (runState == null || runState.status !== "paused") {
        // Idle-first stepping: open the event channel, then arm one turn.
        const sid = newStreamId();
        setActiveStream(sid);
        await startRoom(detail.room.id, null, sid, handleRoomEvent(detail.room.id));
        const state = await stepRoom(detail.room.id);
        setRunState(state);
        setRunStates((prev) => ({ ...prev, [detail.room.id]: state }));
      } else {
        const state = await stepRoom(detail.room.id);
        setRunState(state);
        setRunStates((prev) => ({ ...prev, [detail.room.id]: state }));
      }
    } catch (err: unknown) {
      setActiveStream(null);
      setStreamError(errorMessage(err));
    }
  };

  const doStop = async () => {
    if (!detail) return;
    try {
      const state = await stopRoom(detail.room.id);
      setRunState(state);
      setRunStates((prev) => ({ ...prev, [detail.room.id]: state }));
      setActiveStream(null);
    } catch (err: unknown) {
      setStreamError(errorMessage(err));
    }
  };

  const removeRoom = async (id: string) => {
    setRowBusy(true);
    try {
      await deleteRoom(id);
      setPendingDeleteId(null);
      setRooms((prev) => prev.filter((r) => r.id !== id));
      setMembersByRoom((prev) => {
        if (!(id in prev)) return prev;
        const next = { ...prev };
        delete next[id];
        return next;
      });
      if (selectedIdRef.current === id) {
        selectedIdRef.current = null;
        setSelectedId(null);
        setDetail(null);
        setMessages([]);
        setRunState(null);
      }
    } catch (err: unknown) {
      setRoomsError(errorMessage(err));
    } finally {
      setRowBusy(false);
    }
  };

  const openCreate = () => {
    setFormServerError(null);
    setForm({ mode: "create" });
    setFormKey((k) => k + 1);
  };

  const openEdit = () => {
    if (!detail) return;
    setFormServerError(null);
    setForm({ mode: "edit" });
    setFormKey((k) => k + 1);
  };

  const applyProtocols = async (values: RoomFormValues) => {
    for (const id of values.memberIds) {
      const current = agentsById[id]?.tool_protocol ?? "auto";
      const desired = values.protocols[id] ?? current;
      if (desired !== current) {
        await setAgentProtocol(id, desired);
      }
    }
    await refreshAgents();
  };

  const submitForm = async (values: RoomFormValues) => {
    setFormSaving(true);
    setFormServerError(null);
    try {
      if (form?.mode === "create") {
        const created = await createRoom(
          values.title,
          values.memberIds,
          values.mode,
          values.mode === "orchestrated" ? values.leadAgentId : null,
        );
        const id = created.room.id;
        await updateRoom(id, {
          title: values.title,
          mode: values.mode,
          lead_agent_id: values.mode === "orchestrated" ? values.leadAgentId : null,
          max_turns: values.maxTurns,
          token_budget: values.tokenBudget,
          timeout_ms: values.timeoutSec * 1000,
          loop_detection: values.loopDetection,
        });
        await setRoomMembers(
          id,
          values.memberIds.map((agent_id, seat) => ({
            agent_id,
            seat,
            can_talk_to: values.allowlist[agent_id] ?? ["room"],
          })),
        );
        await applyProtocols(values);
        setForm(null);
        await refreshRooms();
        await selectRoom(id);
      } else if (form?.mode === "edit" && selectedId) {
        await updateRoom(selectedId, {
          title: values.title,
          mode: values.mode,
          lead_agent_id: values.mode === "orchestrated" ? values.leadAgentId : null,
          max_turns: values.maxTurns,
          token_budget: values.tokenBudget,
          timeout_ms: values.timeoutSec * 1000,
          loop_detection: values.loopDetection,
        });
        await setRoomMembers(
          selectedId,
          values.memberIds.map((agent_id, seat) => ({
            agent_id,
            seat,
            can_talk_to: values.allowlist[agent_id] ?? ["room"],
          })),
        );
        await applyProtocols(values);
        setForm(null);
        await refreshRooms();
        await selectRoom(selectedId);
      }
    } catch (err: unknown) {
      setFormServerError(errorMessage(err));
    } finally {
      setFormSaving(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  };

  const filteredRooms = useMemo(() => {
    const query = roomSearch.trim().toLowerCase();
    if (query === "") return rooms;
    return rooms.filter((r) => r.title.toLowerCase().includes(query));
  }, [rooms, roomSearch]);

  const formInitial: RoomFormValues | null = useMemo(() => {
    if (!form) return null;
    if (form.mode === "create") return blankRoomFormValues();
    if (!detail) return null;
    return roomFormValuesFromDetail(detail, agentsById);
  }, [form, detail, agentsById]);

  const agentName = (id: string | null): string =>
    id ? (agentsById[id]?.name ?? "Agent") : "room";

  const renderRouting = (msg: RoomMessage): string | null => {
    if (msg.sender === "system") return null;
    if (msg.sender === "user") return "You → room";
    const from = msg.sender_agent_id ? agentName(msg.sender_agent_id) : "Agent";
    if (msg.recipient_agent_id) return `${from} → ${agentName(msg.recipient_agent_id)}`;
    if (msg.recipient === "room") return `${from} → room`;
    return null; // agent→public: no routing line.
  };

  const renderSender = (msg: RoomMessage): string => {
    if (msg.sender === "system") return "System";
    if (msg.sender === "user") return "You";
    return msg.sender_agent_id ? agentName(msg.sender_agent_id) : "Agent";
  };

  const stopText = runState ? stopReasonText(runState) : null;
  const maxTurns = detail?.room.max_turns ?? 0;
  const tokenBudget = detail?.room.token_budget ?? 0;
  const turnPct =
    runState && maxTurns > 0
      ? Math.min(100, Math.round((runState.turn_count / maxTurns) * 100))
      : 0;
  const tokenPct =
    runState && tokenBudget > 0
      ? Math.min(100, Math.round((runState.tokens_used / tokenBudget) * 100))
      : 0;

  const liveDrafts = Object.entries(drafts).filter(([, text]) => text !== "");

  return (
    <main
      className="pane pane--conversation"
      ref={paneRef}
      tabIndex={-1}
      aria-label="Rooms"
    >
      <header className="pane__header">
        <h1 className="pane__title">Rooms</h1>
        <div className="pane__header-actions">
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
          <button type="button" className="btn" onClick={onBack}>
            Back to chat
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

      <div className="pane__body rooms">
        <div className="agents__toolbar">
          <input
            type="search"
            className="input"
            placeholder="Search rooms…"
            aria-label="Search rooms"
            value={roomSearch}
            onChange={(e) => setRoomSearch(e.target.value)}
          />
          <button type="button" className="btn btn--primary" onClick={openCreate}>
            New room
          </button>
        </div>

        {agentsError && (
          <div className="notice notice--error" role="alert">
            <p className="notice__title">Couldn’t load agents.</p>
            <p className="notice__body">{agentsError}</p>
            <p className="notice__body">
              <button type="button" className="btn" onClick={() => void refreshAgents()}>
                Retry
              </button>
            </p>
          </div>
        )}
        {roomsLoading && (
          <p className="muted" role="status">
            <span className="spinner" aria-hidden="true" /> Loading rooms…
          </p>
        )}
        {roomsError && (
          <div className="notice notice--error" role="alert">
            <p className="notice__title">Couldn’t load rooms.</p>
            <p className="notice__body">{roomsError}</p>
            <p className="notice__body">
              <button type="button" className="btn" onClick={() => void refreshRooms()}>
                Retry
              </button>
            </p>
          </div>
        )}

        <div className="room-layout">
          <div className="room-list-col">
            {!roomsLoading && !roomsError && filteredRooms.length === 0 && (
              <EmptyState
                title={rooms.length === 0 ? "No rooms yet" : "No matches"}
                description={
                  rooms.length === 0
                    ? "Create a room to get several agents talking together."
                    : "Nothing matches that search."
                }
              />
            )}
            <ul className="room-list">
              {filteredRooms.map((room) => {
                const state = runStates[room.id];
                return (
                  <li
                    key={room.id}
                    className={`room-row${selectedId === room.id ? " room-row--active" : ""}`}
                  >
                    <button
                      type="button"
                      className="room-row__main"
                      onClick={() => void selectRoom(room.id)}
                      aria-label={`Open ${room.title}`}
                    >
                      <span className="room-row__text">
                        <span className="room-row__name">{room.title}</span>
                        <span className="room-row__sub">
                          <span className="room-badge">{modeLabel(room.mode)}</span>
                          <span className="room-dots" aria-hidden="true">
                            {(membersByRoom[room.id] ?? []).map((id) => {
                              const agent = agentsById[id];
                              if (!agent) return null;
                              return (
                                <span key={id} title={agent.name}>
                                  <AgentAvatar agent={agent} size={16} />
                                </span>
                              );
                            })}
                          </span>
                          {state && (
                            <span className={`room-status room-status--${state.status}`}>
                              {state.status}
                            </span>
                          )}
                        </span>
                      </span>
                    </button>
                    <span className="room-row__actions">
                      {pendingDeleteId === room.id ? (
                        <button
                          type="button"
                          className="btn btn--danger btn--sm"
                          disabled={rowBusy}
                          onClick={() => void removeRoom(room.id)}
                        >
                          Confirm?
                        </button>
                      ) : (
                        <button
                          type="button"
                          className="btn btn--sm"
                          onClick={() => {
                            setPendingDeleteId(room.id);
                            window.setTimeout(() => {
                              setPendingDeleteId((current) =>
                                current === room.id ? null : current,
                              );
                            }, 4000);
                          }}
                        >
                          Delete
                        </button>
                      )}
                    </span>
                  </li>
                );
              })}
            </ul>

            {form && formInitial && (
              <RoomForm
                key={formKey}
                agents={agents}
                initial={formInitial}
                heading={form.mode === "create" ? "New room" : "Edit room"}
                submitLabel={form.mode === "create" ? "Create" : "Save"}
                saving={formSaving}
                serverError={formServerError}
                onSubmit={(values) => void submitForm(values)}
                onCancel={() => setForm(null)}
              />
            )}
          </div>

          <div className="room-detail-col">
            {selectedId == null && (
              <EmptyState
                title="Pick a room"
                description="Select a room to see its members, run status, and message log."
              />
            )}
            {selectedId != null && detailLoading && (
              <p className="muted" role="status">
                <span className="spinner" aria-hidden="true" /> Loading room…
              </p>
            )}
            {selectedId != null && !detailLoading && detailError && (
              <div className="notice notice--error" role="alert">
                <p className="notice__title">Couldn’t load this room.</p>
                <p className="notice__body">{detailError}</p>
                <p className="notice__body">
                  <button type="button" className="btn" onClick={retryDetail}>
                    Retry
                  </button>
                </p>
              </div>
            )}
            {selectedId != null && !detailLoading && !detailError && detail && runState && (
              <div className="room-detail">
                <div className="room-detail__header">
                  <div className="room-detail__title-row">
                    <h2 className="room-detail__title">{detail.room.title}</h2>
                    <span className="room-badge">{modeLabel(detail.room.mode)}</span>
                    <button
                      type="button"
                      className="btn btn--sm"
                      onClick={openEdit}
                    >
                      Edit
                    </button>
                  </div>
                  <div className="room-members" aria-label="Members">
                    {memberAgents.map((agent) => (
                      <span key={agent.id} className="room-members__item" title={agent.name}>
                        <AgentAvatar agent={agent} size={24} />
                        <span className="room-members__name">{agent.name}</span>
                      </span>
                    ))}
                    {memberAgents.length === 0 && (
                      <span className="muted">No members — edit the room to add agents.</span>
                    )}
                  </div>
                </div>

                <div
                  className="room-runbar"
                  role="status"
                  aria-label={`Run status: ${runState.status}`}
                >
                  <span className={`room-status room-status--${runState.status}`}>
                    {runState.status}
                  </span>
                  <span className="room-runbar__meter" title={`Turns ${runState.turn_count} of ${maxTurns}`}>
                    <span className="room-runbar__label">
                      {runState.turn_count}/{maxTurns} turns
                    </span>
                    <span className="room-progress" aria-hidden="true">
                      <span className="room-progress__fill" style={{ width: `${turnPct}%` }} />
                    </span>
                  </span>
                  <span className="room-runbar__meter" title={`Tokens ${runState.tokens_used} of ${tokenBudget}`}>
                    <span className="room-runbar__label">
                      {runState.tokens_used}/{tokenBudget} tokens
                    </span>
                    <span className="room-progress" aria-hidden="true">
                      <span className="room-progress__fill" style={{ width: `${tokenPct}%` }} />
                    </span>
                  </span>
                  {stopText && <span className="room-runbar__stop">{stopText}</span>}
                </div>

                <div className="room-controls" role="group" aria-label="Run controls">
                  <button
                    type="button"
                    className="btn btn--sm"
                    disabled={!running}
                    onClick={() => void doPause()}
                  >
                    Pause
                  </button>
                  <button
                    type="button"
                    className="btn btn--sm"
                    disabled={runState.status !== "paused"}
                    onClick={() => void doResume()}
                  >
                    Resume
                  </button>
                  <button
                    type="button"
                    className="btn btn--sm"
                    disabled={busy || detailLoading}
                    onClick={() => void doStep()}
                    title="Run a single turn"
                  >
                    Step
                  </button>
                  <button
                    type="button"
                    className="btn btn--sm btn--danger"
                    disabled={runState.status !== "running" && runState.status !== "paused"}
                    onClick={() => void doStop()}
                  >
                    Stop
                  </button>
                </div>
                {approvals.length > 0 && (
                  <p className="muted" role="status">
                    {approvals.length === 1
                      ? "Waiting for approval — 1 tool call needs your decision."
                      : `Waiting for approval — ${approvals.length} tool calls need your decision.`}
                  </p>
                )}

                <div className="card" aria-label="Conversation graph">
                  <h3 className="card__title">Graph</h3>
                  <RoomGraph
                    messages={messages}
                    members={detail.members}
                    agentsById={agentsById}
                  />
                </div>

                <AuditLog
                  key={detail.room.id}
                  roomId={detail.room.id}
                  agentName={(id) => agentName(id)}
                />

                <div
                  className="pane__body chat__scroll room-log"
                  ref={scrollRef}
                  onScroll={(e) => {
                    const el = e.currentTarget;
                    stickRef.current =
                      el.scrollHeight - el.scrollTop - el.clientHeight < 48;
                    onScrollMeas();
                  }}
                >
                  {messages.length === 0 && liveDrafts.length === 0 && !agentsLoading && (
                    <EmptyState
                      title="No messages yet"
                      description="Send a message to start the run. Rooms persist — reopen and continue any time."
                    />
                  )}
                  <ol className="chat__list">
                    {virtualized && vTopPx > 0 && (
                      <div className="virt-spacer" style={{ height: vTopPx }} aria-hidden="true" />
                    )}
                    {visibleMessages.map((msg) => {
                      const routing = renderRouting(msg);
                      const senderAgent =
                        msg.sender === "agent" && msg.sender_agent_id
                          ? (agentsById[msg.sender_agent_id] ?? null)
                          : null;
                      return (
                        <li
                          key={msg.id}
                          className={
                            msg.sender === "user"
                              ? "msg msg--user"
                              : msg.sender === "system"
                                ? "msg room-msg--system"
                                : "msg msg--assistant"
                          }
                        >
                          <div className="msg__head">
                            <span className="msg__role">
                              {senderAgent ? (
                                <>
                                  <AgentAvatar agent={senderAgent} size={18} />{" "}
                                  {senderAgent.name}
                                </>
                              ) : (
                                renderSender(msg)
                              )}
                            </span>
                          </div>
                          {routing && <p className="room-route">{routing}</p>}
                          <p className="msg__text">{msg.content}</p>
                          {msg.tool_calls.length > 0 && (
                            <div className="room-tools">
                              {msg.tool_calls.map((call, index) => (
                                <details
                                  key={call.id === "" ? `pending-${index}` : call.id}
                                  className="room-tool"
                                >
                                  <summary>
                                    {call.tool}
                                    {call.result == null ? " (running…)" : ""}
                                  </summary>
                                  <pre className="room-tool__args">
                                    {JSON.stringify(call.args, null, 2)}
                                  </pre>
                                  {call.result != null && (
                                    <p className="room-tool__result">{call.result}</p>
                                  )}
                                </details>
                              ))}
                            </div>
                          )}
                          <p className="room-meta">
                            {msg.model || "—"} · prompt {msg.prompt_tokens ?? "—"} ·
                            completion {msg.completion_tokens ?? "—"} · total{" "}
                            {msg.total_tokens ?? "—"} ·{" "}
                            {msg.latency_ms != null ? `${msg.latency_ms} ms` : "—"}
                          </p>
                        </li>
                      );
                    })}
                    {virtualized && vBottomPx > 0 && (
                      <div className="virt-spacer" style={{ height: vBottomPx }} aria-hidden="true" />
                    )}
                    {liveDrafts.map(([agentId, text]) => {
                      const agent = agentsById[agentId];
                      if (!agent) return null;
                      return (
                        <li key={`draft-${agentId}`} className="msg msg--assistant" aria-live="polite">
                          <div className="msg__head">
                            <span className="msg__role">
                              <AgentAvatar agent={agent} size={18} /> {agent.name}
                            </span>
                          </div>
                          <p className="msg__text">
                            {text}
                            <span className="chat__cursor" aria-hidden="true" />
                          </p>
                        </li>
                      );
                    })}
                  </ol>
                  {streamError && (
                    <div className="notice notice--error" role="alert">
                      <p className="notice__title">The room run failed.</p>
                      <p className="notice__body">{streamError}</p>
                      <p className="notice__body">
                        <button type="button" className="btn" onClick={retryDetail}>
                          Retry
                        </button>
                      </p>
                    </div>
                  )}
                </div>

                <div className="chat__dock">
                  {directedHint && (
                    <div className="notice" role="note">
                      <p className="notice__body">
                        Directed rooms need a mention — type @ and pick a member agent.
                      </p>
                    </div>
                  )}
                  <form className="chat__composer" onSubmit={onSendSubmit}>
                    <div className="chat__input-wrap">
                      <textarea
                        ref={composerRef}
                        className="textarea chat__input"
                        aria-label="Room message"
                        placeholder={
                          detail.room.mode === "directed"
                            ? "Write a message… (@mention one member to direct it)"
                            : "Write a message… (Enter sends, Shift+Enter adds a line)"
                        }
                        value={composer}
                        onChange={(e) => {
                          setComposer(e.target.value);
                          setDirectedHint(false);
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
                            if (!busy) void send();
                          }
                        }}
                        rows={3}
                        disabled={busy}
                      />
                      {mention && mentionMatches.length > 0 && (
                        <ul className="mention-list" role="listbox" aria-label="Mention a member">
                          {mentionMatches.map((agent, index) => (
                            <li key={agent.id} role="option" aria-selected={index === mentionIndex}>
                              <button
                                type="button"
                                className={`mention-item${index === mentionIndex ? " mention-item--active" : ""}`}
                                onMouseDown={(e) => {
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
                    <button
                      type="submit"
                      className="btn btn--primary chat__send"
                      disabled={busy || composer.trim() === ""}
                    >
                      Send
                    </button>
                  </form>
                </div>
              </div>
            )}
          </div>
        </div>
      </div>
      {approvals.length > 0 && approvals[0] && (
        <ApprovalDialog
          approval={approvals[0]}
          agentName={agentName(approvals[0].agent_id)}
          queueLength={approvals.length}
          onResolved={() => setApprovals((prev) => prev.slice(1))}
        />
      )}
    </main>
  );
}
