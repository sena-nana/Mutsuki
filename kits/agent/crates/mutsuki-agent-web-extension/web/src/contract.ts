import { defineRpc, mutation, query, type JsonValue } from "@mutsuki/web-sdk";

export type ConnectionState = "disabled" | "healthy" | "unavailable" | "reconnecting";

export interface ConnectionStatus {
  connection_id: string;
  connector_id: string;
  generation: number;
  state: ConnectionState;
  negotiated_version?: number | null;
  enabled_features: string[];
  last_error_code?: string | null;
}

export interface ConnectionsSnapshot {
  revision: number;
  connections: ConnectionStatus[];
}

/** Connector config is opaque to the console: secrets stay Host key references inside it. */
export interface ConnectionConfig {
  connection_id: string;
  connector_id: string;
  enabled: boolean;
  config: JsonValue;
}

export type SessionStatus = "active" | "waiting_approval" | "completed" | "cancelled" | "failed" | "closed";

export interface SessionSummary {
  session_id: string;
  title?: string | null;
  turn_count: number;
  message_count: number;
  status: SessionStatus;
}

export interface SessionPage {
  items: SessionSummary[];
  next_session_id?: string | null;
}

export interface AgentMessage {
  role: string;
  content: string;
}

export interface AgentSession {
  session_id: string;
  title?: string | null;
  messages: AgentMessage[];
  turn_count: number;
  next_event_sequence: number;
}

export type TurnStatus =
  | "created"
  | "collecting_context"
  | "generating"
  | "running_tools"
  | "waiting_approval"
  | "completed"
  | "cancelled"
  | "failed";

export interface PermissionRequest {
  session_id: string;
  turn_id: string;
  action_id: string;
  tool: string;
  summary: string;
  version: number;
}

export interface InteractionRequest {
  session_id: string;
  turn_id: string;
  version: number;
  interaction_id: string;
  prompt: string;
}

export interface SessionState {
  session_id: string;
  /** `SessionVersion` is a newtype; older payloads wrap it in a one-element array. */
  version: number | [number];
  status: SessionStatus;
  turns: Array<{ turn_id: string; status: TurnStatus }>;
  pending_approvals: Array<{ request: PermissionRequest }>;
  pending_interactions: InteractionRequest[];
}

export interface AgentEventEnvelope {
  session_id?: string;
  sequence: number;
  meta?: Record<string, JsonValue>;
  event: Record<string, JsonValue> & { type: string };
}

export interface AgentEventPage {
  events: AgentEventEnvelope[];
  next_sequence: number;
  lost: number;
  truncated: boolean;
}

const connectionRead = { capability: "agent.connection.read" };
const connectionWrite = { capability: "agent.connection.write" };
const sessionRead = { capability: "agent.session.read" };
const sessionWrite = { capability: "agent.session.write" };

/** `bot-agent.*` RPCs of `mutsuki-agent-web-extension` (hand-written until typed). */
export const agentApi = defineRpc({
  namespace: "bot-agent",
  methods: {
    "connections.snapshot": query<void, ConnectionsSnapshot>(connectionRead),
    "connections.test": mutation<{ config: ConnectionConfig }, ConnectionStatus>(connectionWrite),
    "connections.upsert": mutation<{ expected_revision: number; config: ConnectionConfig }, ConnectionStatus>(
      connectionWrite,
    ),
    "connections.reconnect": mutation<{ expected_revision: number; connection_id: string }, ConnectionStatus>(
      connectionWrite,
    ),
    "provider.test": mutation<void, { ok: boolean }>(connectionWrite),
    "sessions.list": query<{ limit: number; after_session_id?: string }, SessionPage>(sessionRead),
    "sessions.get": query<{ session_id: string }, AgentSession>(sessionRead),
    "sessions.state": query<{ session_id: string }, SessionState>(sessionRead),
    "sessions.events": query<{ session_id: string; after_sequence: number; limit: number }, AgentEventPage>(
      sessionRead,
    ),
    "sessions.event_get": query<{ session_id: string; sequence: number }, JsonValue>(sessionRead),
    "sessions.approve": mutation<{ decision: Decision }, JsonValue>(sessionWrite),
    "sessions.reject": mutation<{ decision: Decision }, JsonValue>(sessionWrite),
    "sessions.interact": mutation<{ resolution: Resolution }, JsonValue>(sessionWrite),
    "sessions.cancel_turn": mutation<{ session_id: string; turn_id: string; expected_version: number }, JsonValue>(
      sessionWrite,
    ),
    "sessions.close": mutation<{ session_id: string; expected_version: number }, { closed: boolean }>(sessionWrite),
  },
});

export interface Decision {
  session_id: string;
  turn_id: string;
  action_id: string;
  version: number;
  decision: "approved" | "rejected";
}

export interface Resolution {
  session_id: string;
  turn_id: string;
  version: number;
  interaction_id: string;
  accepted: boolean;
  response: JsonValue;
}

export const ACTIVE_TURN: readonly TurnStatus[] = ["collecting_context", "generating", "running_tools"];

export function versionOf(state: SessionState): number {
  return Array.isArray(state.version) ? state.version[0] : state.version;
}

export const EVENT_PAGE_LIMIT = 100;
export const EVENT_WINDOW_LIMIT = EVENT_PAGE_LIMIT * 3;
