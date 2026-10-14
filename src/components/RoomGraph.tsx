import { useMemo } from "react";
import { avatarTextColor } from "./AgentsPage";
import type { Agent, RoomMember, RoomMessage } from "../lib/api";

interface RoomGraphProps {
  messages: RoomMessage[];
  members: RoomMember[];
  agentsById: Record<string, Agent>;
}

const YOU_ID = "__you__";
const WIDTH = 340;
const HEIGHT = 230;

interface GraphNode {
  id: string;
  label: string;
  color: string;
  initial: string;
  x: number;
  y: number;
}

interface GraphEdge {
  key: string;
  from: string;
  to: string;
  count: number;
  broadcast: boolean;
  recent: boolean;
}

/** Circular-layout conversation graph. Pure SVG, no animation, no libraries. */
export default function RoomGraph({ messages, members, agentsById }: RoomGraphProps) {
  const nodes: GraphNode[] = useMemo(() => {
    const ordered = [...members].sort((a, b) => a.seat - b.seat);
    const all: Array<{ id: string; label: string; color: string; initial: string }> = [
      { id: YOU_ID, label: "You", color: "", initial: "Y" },
      ...ordered.map((m) => {
        const agent = agentsById[m.agent_id];
        const name = agent?.name ?? "Agent";
        const icon = agent?.avatar_icon.trim() || name.trim().charAt(0) || "?";
        return {
          id: m.agent_id,
          label: name,
          color: agent?.avatar_color ?? "#5a616e",
          initial: icon,
        };
      }),
    ];
    const cx = WIDTH / 2;
    const cy = HEIGHT / 2 - 6;
    const rx = WIDTH / 2 - 44;
    const ry = HEIGHT / 2 - 40;
    return all.map((n, i) => {
      const angle = -Math.PI / 2 + (i * 2 * Math.PI) / Math.max(all.length, 1);
      return {
        ...n,
        x: cx + rx * Math.cos(angle),
        y: cy + ry * Math.sin(angle),
      };
    });
  }, [members, agentsById]);

  const edges: GraphEdge[] = useMemo(() => {
    const counts = new Map<string, { from: string; to: string; count: number; broadcast: boolean }>();
    let recentKey: string | null = null;
    const memberIds = new Set(members.map((m) => m.agent_id));
    for (const msg of messages) {
      if (msg.sender === "system") continue;
      const from =
        msg.sender === "user" ? YOU_ID : (msg.sender_agent_id ?? null);
      if (!from) continue;
      if (msg.recipient_agent_id) {
        const key = `${from}→${msg.recipient_agent_id}`;
        const existing = counts.get(key);
        if (existing) {
          existing.count += 1;
          existing.broadcast = false;
        } else {
          counts.set(key, { from, to: msg.recipient_agent_id, count: 1, broadcast: false });
        }
        recentKey = key;
      } else if (msg.recipient === "room") {
        // Broadcast: one faint edge to every other member.
        for (const target of memberIds) {
          if (target === from) continue;
          const key = `${from}→${target}`;
          const existing = counts.get(key);
          if (existing) {
            existing.count += 1;
          } else {
            counts.set(key, { from, to: target, count: 1, broadcast: true });
          }
        }
        recentKey = `${from}→room`;
      } else if (msg.sender === "user") {
        // Bare user messages are addressed to the room.
        for (const target of memberIds) {
          const key = `${from}→${target}`;
          const existing = counts.get(key);
          if (existing) {
            existing.count += 1;
          } else {
            counts.set(key, { from, to: target, count: 1, broadcast: true });
          }
        }
        recentKey = `${from}→room`;
      }
      // Agent→public (null recipient) carries no routing: no edge.
    }
    return [...counts.values()].map((e) => ({
      key: `${e.from}→${e.to}`,
      from: e.from,
      to: e.to,
      count: e.count,
      broadcast: e.broadcast,
      recent: recentKey != null && (recentKey === `${e.from}→${e.to}` || recentKey === `${e.from}→room`),
    }));
  }, [messages, members]);

  const byId = useMemo(() => new Map(nodes.map((n) => [n.id, n])), [nodes]);
  const summary = `${nodes.length - 1} member${nodes.length === 2 ? "" : "s"}, ${messages.length} message${messages.length === 1 ? "" : "s"}`;

  const shorten = (x1: number, y1: number, x2: number, y2: number): [number, number, number, number] => {
    const dx = x2 - x1;
    const dy = y2 - y1;
    const len = Math.hypot(dx, dy);
    if (len === 0) return [x1, y1, x2, y2];
    const r = 18 / len;
    return [x1 + dx * r, y1 + dy * r, x2 - dx * r, y2 - dy * r];
  };

  return (
    <div className="room-graph">
      <svg
        className="room-graph__svg"
        viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
        role="img"
        aria-label={`Room conversation graph: ${summary}`}
      >
        <defs>
          <marker id="room-edge" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
            <path d="M 0 1 L 9 5 L 0 9" fill="none" stroke="var(--color-text-muted)" strokeWidth="1.6" />
          </marker>
          <marker id="room-edge-recent" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
            <path d="M 0 1 L 9 5 L 0 9" fill="none" stroke="var(--color-accent)" strokeWidth="1.8" />
          </marker>
        </defs>
        {edges.map((edge) => {
          const a = byId.get(edge.from);
          const b = byId.get(edge.to);
          if (!a || !b) return null;
          const [x1, y1, x2, y2] = shorten(a.x, a.y, b.x, b.y);
          const mx = (x1 + x2) / 2;
          const my = (y1 + y2) / 2;
          return (
            <g key={edge.key}>
              <line
                x1={x1}
                y1={y1}
                x2={x2}
                y2={y2}
                className={
                  edge.recent
                    ? "room-graph__edge room-graph__edge--recent"
                    : edge.broadcast
                      ? "room-graph__edge room-graph__edge--broadcast"
                      : "room-graph__edge"
                }
                markerEnd={edge.recent ? "url(#room-edge-recent)" : "url(#room-edge)"}
              />
              <text x={mx} y={my - 3} textAnchor="middle" className="room-graph__count">
                {edge.count}
              </text>
            </g>
          );
        })}
        {nodes.map((node) => (
          <g key={node.id}>
            <circle
              cx={node.x}
              cy={node.y}
              r="16"
              className={node.id === YOU_ID ? "room-graph__node room-graph__node--you" : "room-graph__node"}
              style={node.id === YOU_ID ? undefined : { fill: node.color }}
            />
            <text x={node.x} y={node.y + 4} textAnchor="middle" className="room-graph__initial" style={node.id === YOU_ID ? undefined : { fill: avatarTextColor(node.color) }}>
              {node.initial}
            </text>
            <text x={node.x} y={node.y + 30} textAnchor="middle" className="room-graph__label">
              {node.label.length > 12 ? `${node.label.slice(0, 11)}…` : node.label}
            </text>
          </g>
        ))}
      </svg>
      <p className="room-graph__legend">
        <span className="room-graph__key room-graph__key--direct">— direct (label = turns)</span>
        <span className="room-graph__key room-graph__key--broadcast">┄ broadcast to room</span>
        <span className="room-graph__key room-graph__key--recent">— latest</span>
      </p>
    </div>
  );
}
