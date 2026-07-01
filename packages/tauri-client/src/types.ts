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

export interface FrontendEventEnvelope<T = MutsukiFrontendEvent> {
  sequence: number;
  channel: string;
  payload: T;
}

export type MutsukiFrontendEvent =
  | { type: "task"; task_id: string; event: RuntimeEvent }
  | { type: "runtime"; event: RuntimeEvent }
  | { type: "trace"; span: JsonValue }
  | { type: "log"; level: string; target: string; message: string }
  | { type: "resource"; ref_id: string; operation: string }
  | { type: "approval"; request: ApprovalRequest }
  | { type: "plugin"; plugin: PluginSummary; operation: string }
  | { type: "runner"; runner_id: string; status: string };

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
  payload: JsonValue;
  context: FrontendContext;
}

export interface ApprovalResponse {
  approval_id: string;
  token: string;
  decision: "allow" | "deny";
  reason?: string;
}

export interface PluginSummary {
  plugin_id: string;
  version: string;
  enabled: boolean;
  deployment: string;
}

export interface HostStatus {
  app_name: string;
  profile_id: string;
  mode: string;
  healthy: boolean;
  plugins: PluginSummary[];
}
