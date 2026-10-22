import { defineRpc, mutation, query, topic } from "@mutsuki/web-sdk";

export interface DeploymentCandidate {
  deployment: string;
  version?: string | null;
  available: boolean;
}

export interface PluginEntry {
  plugin_id: string;
  active_deployment?: string | null;
  preferred_deployment?: string | null;
  candidates?: DeploymentCandidate[];
}

export interface ManifestDiagnostic {
  plugin_id?: string | null;
  manifest_path?: string | null;
  detail?: string | null;
}

export interface PluginList {
  plugins: PluginEntry[];
  diagnostics?: ManifestDiagnostic[];
}

export interface RunnerEntry {
  runner_id: string;
  plugin_id: string;
  state?: string | null;
  pid?: number | null;
  restarts?: number | null;
}

export interface EventSourceEntry {
  source_id: string;
  plugin_id: string;
  state?: string | null;
  health?: string | null;
  reconnects?: number | null;
  last_error?: string | null;
}

export interface TaskEntry {
  task_id: string;
  protocol_id: string;
  status: string;
  runner_hint?: string | null;
  owner_runner?: string | null;
}

export interface RuntimeEvent {
  sequence?: number | null;
  subject_id?: string | number | null;
  kind?: string | null;
  name?: string | null;
  attributes?: Record<string, unknown> | null;
  error?: { message?: string } | null;
}

export interface TaskEventPage {
  events?: RuntimeEvent[];
  items?: RuntimeEvent[];
  earliest_available_sequence?: number | null;
  latest_sequence?: number | null;
  lost?: number | null;
  dropped?: number | null;
  has_more?: boolean;
  truncated?: boolean;
}

export interface LogTail {
  cursor?: number | null;
  entries?: Array<{ offset: number; line: string }>;
}

const read = { capability: "runtime.read" };
const write = { capability: "runtime.write" };

/** `control.*` RPCs of `mutsuki-web-extension-control` (hand-written until B7). */
export const controlApi = defineRpc({
  namespace: "control",
  methods: {
    plugin_list: query<void, PluginList>(read),
    plugin_reload: mutation<void, unknown>(write),
    plugin_deployment_set: mutation<{ plugin_id: string; deployment: string }, unknown>(write),
    plugin_deployment_clear: mutation<{ plugin_id: string }, unknown>(write),
    runner_list: query<void, RunnerEntry[]>(read),
    runner_restart: mutation<{ id: string }, unknown>(write),
    runner_stop: mutation<{ id: string }, unknown>(write),
    event_source_list: query<void, EventSourceEntry[]>(read),
    event_source_restart: mutation<{ id: string }, unknown>(write),
    task_list: query<void, TaskEntry[]>(read),
    task_cancel: mutation<{ id: string }, unknown>(write),
    task_events_after: query<{ sequence: number; limit: number }, TaskEventPage>(read),
    core_begin_drain: mutation<void, unknown>(write),
    service_shutdown: mutation<void, unknown>(write),
    log_tail: query<{ cursor: number | null; lines: number }, LogTail>(read),
  },
});

/** Control-plane change notification, published by the console's control bridge. */
export const controlChanged = topic<{ revision?: number; domains?: string[] }>(
  "control.changed",
  "runtime.read",
);

export type ControlDomain = "plugins" | "runners" | "event_sources" | "tasks" | "logs";
