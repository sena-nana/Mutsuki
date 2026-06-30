import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ApprovalRequest,
  ApprovalResponse,
  FrontendEventEnvelope,
  FrontendTaskRequest,
  FrontendTaskResult,
  HostStatus,
  MutsukiFrontendEvent,
  PluginSummary,
  PreviewHandle,
  ResourceBytes,
  ResourceRef,
  ResourceText,
} from "./types";

export interface MutsukiClient {
  call<TPayload = unknown>(protocolId: string, payload?: TPayload, options?: Partial<FrontendTaskRequest<TPayload>>): Promise<FrontendTaskResult>;
  callStream<TPayload = unknown>(protocolId: string, payload?: TPayload, options?: Partial<FrontendTaskRequest<TPayload>>): Promise<TaskRun>;
  status(): Promise<HostStatus>;
  events: EventApi;
  tasks: TaskApi;
  resources: ResourceApi;
  approvals: ApprovalApi;
  plugins: PluginApi;
}

export interface TaskRun {
  taskId: string;
  result: Promise<FrontendTaskResult>;
  events(): AsyncIterable<FrontendEventEnvelope<MutsukiFrontendEvent>>;
  cancel(reason?: string): Promise<string>;
}

export interface TaskApi {
  call<TPayload = unknown>(protocolId: string, payload?: TPayload, options?: Partial<FrontendTaskRequest<TPayload>>): Promise<FrontendTaskResult>;
  callStream<TPayload = unknown>(protocolId: string, payload?: TPayload, options?: Partial<FrontendTaskRequest<TPayload>>): Promise<TaskRun>;
  cancel(taskId: string, reason?: string): Promise<string>;
}

export interface ResourceApi {
  importFile(path: string): Promise<ResourceRef>;
  readBytes(ref: string | ResourceRef): Promise<Uint8Array>;
  readText(ref: string | ResourceRef): Promise<string>;
  writeBytes(ref: string | ResourceRef, bytes: Uint8Array | number[]): Promise<ResourceRef>;
  exportToFile(ref: string | ResourceRef, target: string): Promise<void>;
  createObjectUrl(ref: string | ResourceRef): Promise<string>;
  createPreview(ref: string | ResourceRef): Promise<PreviewHandle>;
}

export interface ApprovalApi {
  pending(): Promise<ApprovalRequest[]>;
  respond(response: ApprovalResponse): Promise<string>;
  onRequest(handler: (request: ApprovalRequest) => Promise<ApprovalResponse> | ApprovalResponse): Promise<UnlistenFn>;
}

export interface PluginApi {
  list(): Promise<PluginSummary[]>;
}

export interface EventApi {
  listen(handler: (event: FrontendEventEnvelope<MutsukiFrontendEvent>) => void): Promise<UnlistenFn>;
}

export function createMutsukiClient(): MutsukiClient {
  const events: EventApi = {
    listen: (handler) => listen<FrontendEventEnvelope<MutsukiFrontendEvent>>("mutsuki://event", (event) => handler(event.payload)),
  };

  const cancel = (taskId: string, reason?: string) =>
    invoke<string>("mutsuki_cancel_task", { request: { task_id: taskId, reason } });

  const call = async <TPayload = unknown>(
    protocolId: string,
    payload?: TPayload,
    options: Partial<FrontendTaskRequest<TPayload>> = {},
  ) => {
    const request: FrontendTaskRequest<TPayload> = {
      protocol_id: protocolId,
      payload,
      input_refs: [],
      priority: 0,
      ...options,
    };
    return invoke<FrontendTaskResult>("mutsuki_call", { request });
  };

  const callStream = async <TPayload = unknown>(
    protocolId: string,
    payload?: TPayload,
    options: Partial<FrontendTaskRequest<TPayload>> = {},
  ): Promise<TaskRun> => {
    const taskId = options.task_id ?? `frontend-task:${crypto.randomUUID()}`;
    const result = call(protocolId, payload, { ...options, task_id: taskId });
    return {
      taskId,
      result,
      events: () => taskEvents(taskId),
      cancel: (reason?: string) => cancel(taskId, reason),
    };
  };

  const resources: ResourceApi = {
    importFile: (path) => invoke<ResourceRef>("mutsuki_resource_import_file", { path }),
    readBytes: async (ref) => {
      const response = await invoke<ResourceBytes>("mutsuki_resource_read_bytes", { refId: refId(ref) });
      return new Uint8Array(response.bytes);
    },
    readText: async (ref) => {
      const response = await invoke<ResourceText>("mutsuki_resource_read_text", { refId: refId(ref) });
      return response.text;
    },
    writeBytes: (ref, bytes) => invoke<ResourceRef>("mutsuki_resource_write_bytes", { refId: refId(ref), bytes: Array.from(bytes) }),
    exportToFile: (ref, target) => invoke<void>("mutsuki_resource_export_file", { refId: refId(ref), target }),
    createPreview: (ref) => invoke<PreviewHandle>("mutsuki_resource_preview", { refId: refId(ref) }),
    createObjectUrl: async (ref) => {
      const bytes = await resources.readBytes(ref);
      const blob = new Blob([bytes]);
      return URL.createObjectURL(blob);
    },
  };

  const approvals: ApprovalApi = {
    pending: () => invoke<ApprovalRequest[]>("mutsuki_approval_pending"),
    respond: (response) => invoke<string>("mutsuki_approval_respond", { response }),
    onRequest: async (handler) =>
      events.listen(async (event) => {
        if (event.payload.type !== "approval") return;
        const response = await handler(event.payload.request);
        await approvals.respond(response);
      }),
  };

  return {
    call,
    callStream,
    status: () => invoke<HostStatus>("mutsuki_status"),
    events,
    tasks: { call, callStream, cancel },
    resources,
    approvals,
    plugins: {
      list: () => invoke<PluginSummary[]>("mutsuki_plugins_list"),
    },
  };
}

async function* taskEvents(taskId: string): AsyncIterable<FrontendEventEnvelope<MutsukiFrontendEvent>> {
  const queue: FrontendEventEnvelope<MutsukiFrontendEvent>[] = [];
  let notify: (() => void) | undefined;
  const unlisten = await listen<FrontendEventEnvelope<MutsukiFrontendEvent>>("mutsuki://event", (event) => {
    if (event.payload.payload.type === "task" && event.payload.payload.task_id === taskId) {
      queue.push(event.payload);
      notify?.();
      notify = undefined;
    }
  });
  try {
    while (true) {
      if (queue.length === 0) {
        await new Promise<void>((resolve) => {
          notify = resolve;
        });
      }
      while (queue.length > 0) {
        const next = queue.shift();
        if (next) yield next;
      }
    }
  } finally {
    unlisten();
  }
}

function refId(ref: string | ResourceRef): string {
  return typeof ref === "string" ? ref : ref.ref_id;
}
