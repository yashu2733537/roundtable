import { Channel, invoke } from "@tauri-apps/api/core";
import { errorMessage } from "./tauri";

/** The built-in local provider id (Ollama). Never in the providers table. */
export const OLLAMA_ID = "ollama";
export const OLLAMA_V1_BASE = "http://localhost:11434/v1";

export interface ProviderInfo {
  id: string;
  name: string;
  base_url: string;
  created_at: number;
  has_key: boolean;
}

export interface Conversation {
  id: string;
  title: string;
  provider_id: string | null;
  model: string;
  agent_id: string | null;
  created_at: number;
  updated_at: number;
}

export interface ChatMessage {
  id: string;
  conversation_id: string;
  role: "user" | "assistant";
  content: string;
  agent_id: string | null;
  created_at: number;
}

export type ToolProtocol = "auto" | "native" | "json" | "text";

export interface Agent {
  id: string;
  name: string;
  avatar_color: string;
  avatar_icon: string;
  role: string;
  system_prompt: string;
  provider_id: string | null;
  model: string;
  temperature: number;
  max_tokens: number;
  enabled: boolean;
  /** Tool-call wire format. Backend default "auto"; added in Phase 3. */
  tool_protocol: ToolProtocol;
  /** Phase 4: present once the backend lands; treat as absent until then. */
  permissions?: AgentPermissions;
  /** Phase 4: "server:tool" ids assigned to this agent. */
  tools?: string[];
  created_at: number;
  updated_at: number;
}

export interface NewAgent {
  name: string;
  avatar_color: string;
  avatar_icon: string;
  role: string;
  system_prompt: string;
  provider_id: string | null;
  model: string;
  temperature: number;
  max_tokens: number;
  enabled: boolean;
}

export interface AgentTemplate {
  key: string;
  name: string;
  avatar_color: string;
  avatar_icon: string;
  role: string;
  system_prompt: string;
  model: string;
  temperature: number;
  max_tokens: number;
}

export interface RunMessage {
  role: string;
  content: string;
}

export interface RunInfo {
  conversation_id: string;
  model: string;
  provider_id: string | null;
  agent_id: string | null;
  request_json: string;
  prompt_tokens: number | null;
  completion_tokens: number | null;
  total_tokens: number | null;
  latency_ms: number;
  created_at: number;
}

/** Parse the exact request payload stored with a run. Never throws. */
export function parseRunMessages(run: RunInfo | null): RunMessage[] {
  if (!run) return [];
  try {
    const parsed: unknown = JSON.parse(run.request_json);
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter(
        (m): m is RunMessage =>
          typeof m === "object" &&
          m !== null &&
          typeof (m as RunMessage).role === "string" &&
          typeof (m as RunMessage).content === "string",
      )
      .map((m) => ({ role: m.role, content: m.content }));
  } catch {
    return [];
  }
}

export interface OllamaStatus {
  running: boolean;
  models: string[];
}

export type StreamEvent =
  | { type: "token"; delta: string }
  | {
      type: "done";
      content: string;
      message_id: string;
      stopped: boolean;
      agent_id: string | null;
    }
  | { type: "error"; message: string };

export interface ProviderInput {
  name: string;
  base_url: string;
  /** undefined = leave stored key untouched; string (possibly "") = set/clear. */
  key?: string;
}

export function newStreamId(): string {
  return (
    typeof crypto !== "undefined" && "randomUUID" in crypto
      ? crypto.randomUUID()
      : `stream-${Date.now()}-${Math.floor(Math.random() * 1e9)}`
  );
}

// -- providers ---------------------------------------------------------------

export function listProviders(): Promise<ProviderInfo[]> {
  return invoke<ProviderInfo[]>("providers_list");
}

export function addProvider(input: ProviderInput): Promise<ProviderInfo> {
  return invoke<ProviderInfo>("providers_add", { input });
}

export function updateProvider(id: string, input: ProviderInput): Promise<ProviderInfo> {
  return invoke<ProviderInfo>("providers_update", { id, input });
}

export function removeProvider(id: string): Promise<void> {
  return invoke<void>("providers_remove", { id });
}

export function providerKeyHas(id: string): Promise<boolean> {
  return invoke<boolean>("provider_key_has", { id });
}

export function testProvider(id: string): Promise<string> {
  return invoke<string>("providers_test", { id });
}

export function providerModels(id: string): Promise<string[]> {
  return invoke<string[]>("provider_models", { id });
}

export function fetchOllamaStatus(): Promise<OllamaStatus> {
  return invoke<OllamaStatus>("ollama_status");
}

// -- conversations -------------------------------------------------------------

export function listConversations(): Promise<Conversation[]> {
  return invoke<Conversation[]>("conversations_list");
}

export function searchConversations(query: string): Promise<Conversation[]> {
  return invoke<Conversation[]>("conversations_search", { query });
}

export function createConversation(
  title: string,
  provider_id: string | null,
  model: string,
): Promise<Conversation> {
  return invoke<Conversation>("conversations_create", {
    title,
    provider_id,
    model,
  });
}

export function renameConversation(id: string, title: string): Promise<Conversation> {
  return invoke<Conversation>("conversations_rename", { id, title });
}

export function deleteConversation(id: string): Promise<void> {
  return invoke<void>("conversations_delete", { id });
}

export function getConversation(id: string): Promise<Conversation | null> {
  return invoke<Conversation | null>("conversation_get", { id });
}

export function setConversationModel(
  id: string,
  provider_id: string | null,
  model: string,
): Promise<Conversation> {
  return invoke<Conversation>("conversations_set_model", {
    id,
    provider_id,
    model,
  });
}

export function listMessages(conversation_id: string): Promise<ChatMessage[]> {
  return invoke<ChatMessage[]>("messages_list", { conversation_id });
}

// -- streaming chat --------------------------------------------------------------

export function sendChat(
  conversation_id: string,
  content: string,
  stream_id: string,
  onEvent: (event: StreamEvent) => void,
  agent_id?: string | null,
): Promise<string> {
  const channel = new Channel<StreamEvent>(onEvent);
  return invoke<string>("chat_send", {
    channel,
    conversation_id,
    content,
    stream_id,
    agent_id: agent_id ?? null,
  });
}

export function regenerateChat(
  conversation_id: string,
  stream_id: string,
  onEvent: (event: StreamEvent) => void,
  agent_id?: string | null,
): Promise<string> {
  const channel = new Channel<StreamEvent>(onEvent);
  return invoke<string>("chat_regenerate", {
    channel,
    conversation_id,
    stream_id,
    agent_id: agent_id ?? null,
  });
}

export function editResendChat(
  conversation_id: string,
  message_id: string,
  content: string,
  stream_id: string,
  onEvent: (event: StreamEvent) => void,
  agent_id?: string | null,
): Promise<string> {
  const channel = new Channel<StreamEvent>(onEvent);
  return invoke<string>("chat_edit_resend", {
    channel,
    conversation_id,
    message_id,
    content,
    stream_id,
    agent_id: agent_id ?? null,
  });
}

export function cancelChat(stream_id: string): Promise<void> {
  return invoke<void>("chat_cancel", { stream_id });
}

// -- agents --------------------------------------------------------------------

export function searchAgents(query: string, enabled?: boolean): Promise<Agent[]> {
  return invoke<Agent[]>("agents_search", {
    query,
    enabled: enabled ?? null,
  });
}

export function getAgent(id: string): Promise<Agent | null> {
  return invoke<Agent | null>("agents_get", { id });
}

export function createAgent(draft: NewAgent): Promise<Agent> {
  return invoke<Agent>("agents_create", { draft });
}

export function updateAgent(id: string, draft: NewAgent): Promise<Agent> {
  return invoke<Agent>("agents_update", { id, draft });
}

export function deleteAgent(id: string): Promise<void> {
  return invoke<void>("agents_delete", { id });
}

export function duplicateAgent(id: string): Promise<Agent> {
  return invoke<Agent>("agents_duplicate", { id });
}

export function exportAgent(id: string): Promise<string> {
  return invoke<string>("agents_export", { id });
}

export function importAgent(json: string): Promise<Agent> {
  return invoke<Agent>("agents_import", { json });
}

export function fetchAgentTemplates(): Promise<AgentTemplate[]> {
  return invoke<AgentTemplate[]>("agent_templates");
}

export function testAgent(
  draft: NewAgent,
  content: string,
  stream_id: string,
  onEvent: (event: StreamEvent) => void,
): Promise<string> {
  const channel = new Channel<StreamEvent>(onEvent);
  return invoke<string>("agent_test", { channel, draft, content, stream_id });
}

export function setConversationAgent(
  id: string,
  agent_id: string | null,
): Promise<Conversation> {
  return invoke<Conversation>("conversations_set_agent", { id, agent_id });
}

export function fetchRunInfo(conversation_id: string): Promise<RunInfo | null> {
  return invoke<RunInfo | null>("run_info", { conversation_id });
}

// -- rooms (Phase 3: multi-agent rooms) ------------------------------------------
// FROZEN CONTRACT: the backend implements these exact Tauri command names and
// JSON shapes; the rooms UI codes against these wrappers. Do not change
// signatures without updating both sides.

export type RoomMode = "directed" | "orchestrated" | "round_robin";
export type RoomSender = "user" | "agent" | "system";
export type RoomRecipient = "agent" | "room" | null;
export type RoomStatus = "idle" | "running" | "paused" | "stopped" | "done";

export interface Room {
  id: string;
  title: string;
  mode: RoomMode;
  lead_agent_id: string | null;
  max_turns: number;
  token_budget: number;
  timeout_ms: number;
  loop_detection: boolean;
  created_at: number;
  updated_at: number;
}

export interface RoomMember {
  room_id: string;
  agent_id: string;
  seat: number;
  /** Agent ids this member may message; "room" = may broadcast to the room. */
  can_talk_to: string[];
}

export interface RoomDetail {
  room: Room;
  members: RoomMember[];
}

export interface RoomToolCall {
  id: string;
  tool: "send_message" | "delegate" | "finish";
  args: Record<string, unknown>;
  /** Filled after execution; null while the call is in flight. */
  result: string | null;
}

export interface RoomMessage {
  id: string;
  room_id: string;
  sender: RoomSender;
  sender_agent_id: string | null;
  recipient: RoomRecipient;
  recipient_agent_id: string | null;
  content: string;
  tool_calls: RoomToolCall[];
  model: string;
  prompt_tokens: number | null;
  completion_tokens: number | null;
  total_tokens: number | null;
  latency_ms: number | null;
  created_at: number;
}

export interface RoomRunState {
  room_id: string;
  status: RoomStatus;
  /** Machine-readable reason when status is stopped/done, e.g. "turn_limit". */
  stop_reason: string | null;
  turn_count: number;
  tokens_used: number;
  updated_at: number;
}

export interface RoomPatch {
  title?: string;
  mode?: RoomMode;
  lead_agent_id?: string | null;
  max_turns?: number;
  token_budget?: number;
  timeout_ms?: number;
  loop_detection?: boolean;
}

export interface RoomMemberInput {
  agent_id: string;
  seat: number;
  can_talk_to: string[];
}

export type RoomEvent =
  | { type: "token"; agent_id: string; delta: string }
  | { type: "message"; message: RoomMessage }
  | { type: "tool_call"; agent_id: string; call: RoomToolCall }
  | {
      type: "tool_result";
      agent_id: string;
      call_id: string;
      tool: string;
      result: string;
    }
  | { type: "run_state"; state: RoomRunState }
  | {
      type: "approval";
      call_id: string;
      agent_id: string;
      tool: string;
      summary: string;
      command: string | null;
    }
  | { type: "error"; message: string };

export function listRooms(): Promise<Room[]> {
  return invoke<Room[]>("rooms_list");
}

export function createRoom(
  title: string,
  member_ids: string[],
  mode: RoomMode,
  lead_agent_id?: string | null,
): Promise<RoomDetail> {
  return invoke<RoomDetail>("rooms_create", {
    title,
    member_ids,
    mode,
    lead_agent_id: lead_agent_id ?? null,
  });
}

export function getRoom(id: string): Promise<RoomDetail | null> {
  return invoke<RoomDetail | null>("rooms_get", { id });
}

export function updateRoom(id: string, patch: RoomPatch): Promise<RoomDetail> {
  return invoke<RoomDetail>("rooms_update", { id, patch });
}

export function deleteRoom(id: string): Promise<void> {
  return invoke<void>("rooms_delete", { id });
}

export function setRoomMembers(
  id: string,
  members: RoomMemberInput[],
): Promise<RoomDetail> {
  return invoke<RoomDetail>("rooms_set_members", { id, members });
}

export function listRoomMessages(room_id: string): Promise<RoomMessage[]> {
  return invoke<RoomMessage[]>("room_messages_list", { room_id });
}

/**
 * Append `content` as a user message (when non-empty) and run the scheduler.
 * Resolves when the invoke is accepted; progress arrives via `onEvent`.
 * Rejects if a run is already in flight for the room.
 */
export function startRoom(
  room_id: string,
  content: string | null,
  stream_id: string,
  onEvent: (event: RoomEvent) => void,
  step_once?: boolean,
): Promise<string> {
  const channel = new Channel<RoomEvent>(onEvent);
  return invoke<string>("rooms_start", {
    channel,
    room_id,
    content,
    stream_id,
    step_once: step_once ?? null,
  });
}

export function pauseRoom(room_id: string): Promise<RoomRunState> {
  return invoke<RoomRunState>("rooms_pause", { room_id });
}

export function resumeRoom(room_id: string): Promise<RoomRunState> {
  return invoke<RoomRunState>("rooms_resume", { room_id });
}

export function stepRoom(room_id: string): Promise<RoomRunState> {
  return invoke<RoomRunState>("rooms_step", { room_id });
}

export function stopRoom(room_id: string): Promise<RoomRunState> {
  return invoke<RoomRunState>("rooms_stop", { room_id });
}

export function fetchRoomRunState(room_id: string): Promise<RoomRunState> {
  return invoke<RoomRunState>("rooms_run_state", { room_id });
}

export function setAgentProtocol(
  agent_id: string,
  protocol: ToolProtocol,
): Promise<Agent> {
  return invoke<Agent>("agents_set_protocol", { agent_id, protocol });
}

// -- phase 4: external tools, permissions, remote -------------------------------
// FROZEN CONTRACT (Phase 4): same rules as rooms — the backend implements
// these exact Tauri command names and JSON shapes; the UI codes against
// these wrappers. Do not change signatures without updating both sides.

export interface AgentPermissions {
  workspace: boolean;
  network: boolean;
  shell: boolean;
}

export const NO_PERMISSIONS: AgentPermissions = {
  workspace: false,
  network: false,
  shell: false,
};

export interface McpServer {
  id: string;
  name: string;
  transport: "stdio" | "http";
  command: string | null;
  args: string[];
  url: string | null;
  enabled: boolean;
}

export interface McpTool {
  name: string;
  description: string;
  input_schema: Record<string, unknown>;
}

export interface AuditRow {
  id: number;
  created_at: number;
  room_id: string | null;
  agent_id: string | null;
  tool: string;
  args_json: string;
  decision: string;
  result_summary: string;
}

export type ApprovalDecision = "once" | "always" | "deny";

export interface RemoteStatus {
  running: boolean;
  bind: string;
  port: number;
  tls: boolean;
  token_count: number;
}

export interface RemoteToken {
  id: string;
  label: string;
  created_at: number;
}

export interface RemoteClientStatus {
  configured: boolean;
  url: string | null;
  has_token: boolean;
}

export function mcpServers(): Promise<McpServer[]> {
  return invoke<McpServer[]>("mcp_servers");
}

export function mcpAddServer(input: {
  name: string;
  transport: "stdio" | "http";
  command?: string;
  args?: string[];
  url?: string;
}): Promise<McpServer> {
  return invoke<McpServer>("mcp_add_server", { input });
}

export function mcpRemoveServer(id: string): Promise<void> {
  return invoke<void>("mcp_remove_server", { id });
}

export function mcpSetEnabled(id: string, enabled: boolean): Promise<McpServer> {
  return invoke<McpServer>("mcp_set_enabled", { id, enabled });
}

export function mcpTools(server_id: string): Promise<McpTool[]> {
  return invoke<McpTool[]>("mcp_tools", { server_id });
}

export function agentTools(agent_id: string): Promise<string[]> {
  return invoke<string[]>("agents_tools", { agent_id });
}

export function setAgentTools(agent_id: string, tools: string[]): Promise<string[]> {
  return invoke<string[]>("agents_set_tools", { agent_id, tools });
}

export function agentPermissions(agent_id: string): Promise<AgentPermissions> {
  return invoke<AgentPermissions>("agents_permissions", { agent_id });
}

export function setAgentPermissions(
  agent_id: string,
  perms: AgentPermissions,
): Promise<AgentPermissions> {
  return invoke<AgentPermissions>("agents_set_permissions", {
    agent_id,
    perms,
  });
}

export function workspaceDir(): Promise<string | null> {
  return invoke<string | null>("workspace_dir");
}

export function pickWorkspaceDir(): Promise<string | null> {
  return invoke<string | null>("workspace_pick_dir");
}

export function clearWorkspaceDir(): Promise<void> {
  return invoke<void>("workspace_clear_dir");
}

export function setWebSearchKey(key: string): Promise<void> {
  return invoke<void>("tools_set_search_key", { key });
}

export function hasWebSearchKey(): Promise<boolean> {
  return invoke<boolean>("tools_search_key_has");
}

export function auditList(
  room_id?: string | null,
  limit?: number,
): Promise<AuditRow[]> {
  return invoke<AuditRow[]>("audit_list", {
    room_id: room_id ?? null,
    limit: limit ?? 100,
  });
}

export function approveCall(call_id: string, decision: ApprovalDecision): Promise<void> {
  return invoke<void>("rooms_approve", { call_id, decision });
}

export function remoteStatus(): Promise<RemoteStatus> {
  return invoke<RemoteStatus>("remote_status");
}

export function remoteStart(bind: string, port: number): Promise<RemoteStatus> {
  return invoke<RemoteStatus>("remote_start", { bind, port });
}

export function remoteStop(): Promise<RemoteStatus> {
  return invoke<RemoteStatus>("remote_stop");
}

export function remotePairingCode(): Promise<{ code: string; expires_at: number }> {
  return invoke<{ code: string; expires_at: number }>("remote_pairing_code");
}

export function remoteTokens(): Promise<RemoteToken[]> {
  return invoke<RemoteToken[]>("remote_tokens");
}

export function remoteRevoke(id: string): Promise<void> {
  return invoke<void>("remote_revoke", { id });
}

export function remoteClientStatus(): Promise<RemoteClientStatus> {
  return invoke<RemoteClientStatus>("remote_client_status");
}

export function remoteConnect(url: string, code: string, label: string): Promise<void> {
  return invoke<void>("remote_connect", { url, code, label });
}

export function remoteDisconnect(): Promise<void> {
  return invoke<void>("remote_disconnect");
}

// -- settings + data -------------------------------------------------------------

export function getSettings(): Promise<Record<string, string>> {
  return invoke<Record<string, string>>("settings_get");
}

export function setSettings(pairs: Record<string, string>): Promise<void> {
  return invoke<void>("settings_set", { pairs });
}

export function exportData(): Promise<string> {
  return invoke<string>("data_export");
}

export function importData(json: string): Promise<string> {
  return invoke<string>("data_import", { json });
}

export function fetchDataDir(): Promise<string> {
  return invoke<string>("data_dir");
}

export function openDataDir(): Promise<void> {
  return invoke<void>("open_data_dir");
}

export { errorMessage };
