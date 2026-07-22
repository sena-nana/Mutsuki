export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };

export interface FrontendContext {
  window_label?: string;
  webview_id?: string;
  session_id?: string;
  user_action_id?: string;
}

export interface FrontendTaskRequest<TPayload = JsonValue> {
  protocol_id: string;
  payload?: TPayload;
  task_id?: string;
  trace_id?: string;
  correlation_id?: string;
  idempotency_key?: string;
  input_refs?: string[];
  priority?: number;
  context?: FrontendContext;
}

export interface FrontendTaskResult {
  task_id: string;
  status?: string;
  outcome?: JsonValue;
  events: RuntimeEvent[];
  events_dropped: number;
  events_truncated: boolean;
}

export interface FrontendTaskRun {
  task_id: string;
}

export interface TaskResultRequest {
  task_id: string;
}

export interface RuntimeEvent {
  sequence: number;
  kind: string;
  name: string;
  subject_id?: string | null;
  attributes: Record<string, JsonValue>;
  error?: JsonValue | null;
}

export type SpanStatus = "ok" | "error";

export interface TraceSpan {
  trace_id: string;
  span_id: string;
  parent_span_id?: string | null;
  name: string;
  start: number;
  end?: number | null;
  attributes: Record<string, JsonValue>;
  status: SpanStatus;
}

export interface FrontendLogRecord {
  level: string;
  target: string;
  message: string;
  timestamp_ms: number;
  trace_id?: string | null;
  correlation_id?: string | null;
  fields: Record<string, JsonValue>;
}

export interface FrontendEventEnvelope<T = MutsukiFrontendEvent> {
  sequence: number;
  channel: string;
  payload: T;
}

export type MutsukiFrontendEvent =
  | { type: "batch"; events: MutsukiFrontendEvent[] }
  | { type: "observability_gap"; stream: string; lost: number; dropped: number }
  | { type: "task"; task_id: string; event: RuntimeEvent }
  | { type: "runtime"; event: RuntimeEvent }
  | { type: "trace"; span: TraceSpan }
  | { type: "log"; record: FrontendLogRecord }
  | { type: "resource"; ref_id: string; operation: string }
  | { type: "approval"; request: ApprovalRequest }
  | { type: "plugin"; plugin: PluginSummary; operation: string }
  | { type: "runner"; runner_id: string; status: string }
  | { type: "app_delivery"; progress: AppDeliveryProgress };

export interface AppDeliveryProgress {
  request_id: string;
  target_app: string;
  phase:
    | "draft_saved"
    | "connecting"
    | "target_activating"
    | "target_ready"
    | "negotiating"
    | "transmitting"
    | "accepted"
    | "completed"
    | "delivery_failed";
  error?: string | null;
  error_kind?: string | null;
}

export type TaskFrontendEvent = Extract<MutsukiFrontendEvent, { type: "task" }>;
export type RuntimeFrontendEvent = Extract<MutsukiFrontendEvent, { type: "runtime" }>;
export type TraceFrontendEvent = Extract<MutsukiFrontendEvent, { type: "trace" }>;
export type LogFrontendEvent = Extract<MutsukiFrontendEvent, { type: "log" }>;
export type PluginFrontendEvent = Extract<MutsukiFrontendEvent, { type: "plugin" }>;
export type RunnerFrontendEvent = Extract<MutsukiFrontendEvent, { type: "runner" }>;
export type AppDeliveryFrontendEvent = Extract<MutsukiFrontendEvent, { type: "app_delivery" }>;

export interface ResourceRef {
  ref_id: string;
  resource_id: JsonValue;
  semantic: string;
  provider_id: string;
  resource_kind: string;
  schema: string;
  version: number;
  generation: number;
  access: JsonValue;
  size_hint?: number | null;
  content_hash?: string | null;
  lifetime: JsonValue;
  lease?: JsonValue | null;
  seal_state: string;
}

export interface ResourceBytes {
  resource: ResourceRef;
  bytes: number[];
  media_type?: string | null;
}

export interface ResourceChunk {
  resource: ResourceRef;
  offset: number;
  total_bytes: number;
  bytes: number[];
  eof: boolean;
}

export interface ResourceText {
  ref_id: string;
  text: string;
}

export interface PreviewHandle {
  ref_id: string;
  token: string;
  url: string;
  expires_at_unix_secs: number;
}

export interface ApprovalRequest {
  approval_id: string;
  token: string;
  requester: string;
  operation: string;
  risk: string;
  trace_id: string;
  correlation_id: string;
  payload: JsonValue;
  context: FrontendContext;
}

export interface ApprovalDecisionInput {
  decision: "allow" | "deny";
  reason?: string;
}

export interface ApprovalResponse {
  approval_id: string;
  token: string;
  decision: "allow" | "deny";
  reason?: string;
  trace_id?: string;
  correlation_id?: string;
  context?: FrontendContext;
}

export interface PluginSummary {
  plugin_id: string;
  version: string;
  enabled: boolean;
  deployment: string;
  status: string;
  error?: string | null;
}

export interface RunnerSummary {
  runner_id: string;
  plugin_id: string;
  enabled: boolean;
  deployment: string;
  status: string;
  error?: string | null;
}

export interface HealthComponent {
  healthy: boolean;
  status: string;
  error?: string | null;
}

export interface RuntimeHealth {
  healthy: boolean;
  status: string;
  active_tasks: number;
  failed_tasks: number;
  error?: string | null;
}

export interface HostRecentError {
  source: string;
  message: string;
  timestamp_ms: number;
  plugin_id?: string | null;
  runner_id?: string | null;
  task_id?: string | null;
  code?: string | null;
  route?: string | null;
}

export interface HostStatus {
  app_name: string;
  profile_id: string;
  mode: string;
  healthy: boolean;
  runtime: RuntimeHealth;
  host: HealthComponent;
  plugins_health: HealthComponent;
  runners_health: HealthComponent;
  recent_errors: HostRecentError[];
  plugins: PluginSummary[];
  runners: RunnerSummary[];
}
