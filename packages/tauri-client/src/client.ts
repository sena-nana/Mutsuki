import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ApprovalDecisionInput,
  ApprovalRequest,
  ApprovalResponse,
  FrontendEventEnvelope,
  FrontendTaskRequest,
  FrontendTaskResult,
  FrontendTaskRun,
  HostStatus,
  LogFrontendEvent,
  MutsukiFrontendEvent,
  PluginFrontendEvent,
  PluginSummary,
  PreviewHandle,
  ResourceChunk,
  ResourceRef,
  ResourceText,
  RunnerFrontendEvent,
  RunnerSummary,
  RuntimeFrontendEvent,
  TaskFrontendEvent,
  TraceFrontendEvent,
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
  runners: RunnerApi;
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
  peekResult(taskId: string): Promise<FrontendTaskResult>;
}

export interface ResourceApi {
  importFile(path: string): Promise<ResourceRef>;
  readBytes(ref: string | ResourceRef): Promise<Uint8Array>;
  readText(ref: string | ResourceRef): Promise<string>;
  writeBytes(ref: string | ResourceRef, bytes: Uint8Array | number[]): Promise<ResourceRef>;
  exportToFile(ref: string | ResourceRef, target: string): Promise<void>;
  createObjectUrl(ref: string | ResourceRef): Promise<string>;
  createPreview(ref: string | ResourceRef): Promise<PreviewHandle>;
  releasePreview(handle: PreviewHandle): Promise<void>;
}

export interface ApprovalApi {
  pending(): Promise<ApprovalRequest[]>;
  respond(response: ApprovalResponse): Promise<string>;
  respondTo(request: ApprovalRequest, decision: ApprovalResponse | ApprovalDecisionInput): Promise<string>;
  onRequest(
    handler: (request: ApprovalRequest) => Promise<ApprovalResponse | ApprovalDecisionInput> | ApprovalResponse | ApprovalDecisionInput,
  ): Promise<UnlistenFn>;
}

export interface PluginApi {
  list(): Promise<PluginSummary[]>;
}

export interface RunnerApi {
  list(): Promise<RunnerSummary[]>;
}

export interface EventApi {
  listen(handler: (event: FrontendEventEnvelope<MutsukiFrontendEvent>) => void): Promise<UnlistenFn>;
  tasks(handler: (event: FrontendEventEnvelope<TaskFrontendEvent>) => void): Promise<UnlistenFn>;
  runtime(handler: (event: FrontendEventEnvelope<RuntimeFrontendEvent>) => void): Promise<UnlistenFn>;
  trace(handler: (event: FrontendEventEnvelope<TraceFrontendEvent>) => void): Promise<UnlistenFn>;
  log(handler: (event: FrontendEventEnvelope<LogFrontendEvent>) => void): Promise<UnlistenFn>;
  plugins(handler: (event: FrontendEventEnvelope<PluginFrontendEvent>) => void): Promise<UnlistenFn>;
  runners(handler: (event: FrontendEventEnvelope<RunnerFrontendEvent>) => void): Promise<UnlistenFn>;
}

export function createMutsukiClient(): MutsukiClient {
  const events: EventApi = {
    listen: (handler) => listenAllEvents(handler),
    tasks: (handler) => listenCategory<TaskFrontendEvent>("mutsuki://task/event", handler),
    runtime: (handler) => listenCategory<RuntimeFrontendEvent>("mutsuki://runtime/event", handler),
    trace: (handler) => listenCategory<TraceFrontendEvent>("mutsuki://trace/event", handler),
    log: (handler) => listenCategory<LogFrontendEvent>("mutsuki://log/event", handler),
    plugins: (handler) => listenCategory<PluginFrontendEvent>("mutsuki://plugin/event", handler),
    runners: (handler) => listenCategory<RunnerFrontendEvent>("mutsuki://runner/event", handler),
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
    const request: FrontendTaskRequest<TPayload> = {
      protocol_id: protocolId,
      payload,
      input_refs: [],
      priority: 0,
      ...options,
      task_id: taskId,
    };
    const stream = await openTaskEvents(taskId);
    try {
      await invoke<FrontendTaskRun>("mutsuki_start_task", { request });
    } catch (error) {
      stream.close();
      throw error;
    }
    const result = invoke<FrontendTaskResult>("mutsuki_task_result", { request: { task_id: taskId } });
    void result.then(
      () => stream.close(),
      () => stream.close(),
    );
    return {
      taskId,
      result,
      events: () => stream.events(),
      cancel: (reason?: string) => cancel(taskId, reason),
    };
  };

  const resources: ResourceApi = {
    importFile: (path) => invoke<ResourceRef>("mutsuki_resource_import_file", { path }),
    readBytes: async (ref) => {
      const chunks: number[] = [];
      let offset = 0;
      for (;;) {
        const response = await invoke<ResourceChunk>("mutsuki_resource_read_chunk", {
          refId: refId(ref),
          offset,
          length: 64 * 1024,
        });
        chunks.push(...response.bytes);
        offset += response.bytes.length;
        if (response.eof) return new Uint8Array(chunks);
      }
    },
    readText: async (ref) => {
      const response = await invoke<ResourceText>("mutsuki_resource_read_text", { refId: refId(ref) });
      return response.text;
    },
    writeBytes: (ref, bytes) => invoke<ResourceRef>("mutsuki_resource_write_bytes", { refId: refId(ref), bytes: Array.from(bytes) }),
    exportToFile: (ref, target) => invoke<void>("mutsuki_resource_export_file", { refId: refId(ref), target }),
    createPreview: (ref) => invoke<PreviewHandle>("mutsuki_resource_preview", { refId: refId(ref) }),
    releasePreview: (handle) => invoke<void>("mutsuki_resource_preview_release", { token: handle.token }),
    createObjectUrl: async (ref) => {
      const preview = await resources.createPreview(ref);
      return preview.url;
    },
  };

  const approvals: ApprovalApi = {
    pending: () => invoke<ApprovalRequest[]>("mutsuki_approval_pending"),
    respond: (response) => invoke<string>("mutsuki_approval_respond", { response }),
    respondTo: (request, decision) => approvals.respond(approvalResponseForRequest(request, decision)),
    onRequest: async (handler) =>
      events.listen(async (event) => {
        if (event.payload.type !== "approval") return;
        const request = event.payload.request;
        const response = await handler(request);
        await approvals.respondTo(request, response);
      }),
  };

  return {
    call,
    callStream,
    status: () => invoke<HostStatus>("mutsuki_status"),
    events,
    tasks: {
      call,
      callStream,
      cancel,
      peekResult: (taskId) =>
        invoke<FrontendTaskResult>("mutsuki_peek_task_result", { request: { task_id: taskId } }),
    },
    resources,
    approvals,
    plugins: {
      list: () => invoke<PluginSummary[]>("mutsuki_plugins_list"),
    },
    runners: {
      list: () => invoke<RunnerSummary[]>("mutsuki_runners_list"),
    },
  };
}

interface TaskEventStream {
  events(): AsyncIterable<FrontendEventEnvelope<MutsukiFrontendEvent>>;
  close(): void;
}

async function openTaskEvents(taskId: string): Promise<TaskEventStream> {
  const queue: FrontendEventEnvelope<MutsukiFrontendEvent>[] = [];
  const waiters: Array<() => void> = [];
  let closed = false;
  const notify = () => {
    const pending = waiters.splice(0);
    for (const resolve of pending) resolve();
  };
  const unlisten = await listenAllEvents((envelope) => {
    if (envelope.payload.type === "task" && envelope.payload.task_id === taskId) {
      queue.push(envelope);
      notify();
    }
  });

  return {
    async *events() {
      while (queue.length > 0 || !closed) {
        if (queue.length === 0) {
          await new Promise<void>((resolve) => {
            waiters.push(resolve);
          });
          continue;
        }
        while (queue.length > 0) {
          const next = queue.shift();
          if (next) yield next;
        }
      }
    },
    close() {
      if (closed) return;
      closed = true;
      unlisten();
      notify();
    },
  };
}

function refId(ref: string | ResourceRef): string {
  return typeof ref === "string" ? ref : ref.ref_id;
}

function approvalResponseForRequest(
  request: ApprovalRequest,
  decision: ApprovalResponse | ApprovalDecisionInput,
): ApprovalResponse {
  return {
    ...decision,
    approval_id: request.approval_id,
    token: request.token,
    trace_id: request.trace_id,
    correlation_id: request.correlation_id,
    context: request.context,
  };
}

function listenCategory<T extends MutsukiFrontendEvent>(
  channel: string,
  handler: (event: FrontendEventEnvelope<T>) => void,
): Promise<UnlistenFn> {
  return listenAllEvents((event) => {
    if (event.channel === channel) handler(event as FrontendEventEnvelope<T>);
  });
}

async function listenAllEvents(
  handler: (event: FrontendEventEnvelope<MutsukiFrontendEvent>) => void,
): Promise<UnlistenFn> {
  return listen<FrontendEventEnvelope<MutsukiFrontendEvent>>("mutsuki://event", (event) => {
    for (const envelope of flattenEnvelope(event.payload)) handler(envelope);
  });
}

function flattenEnvelope(
  envelope: FrontendEventEnvelope<MutsukiFrontendEvent>,
): FrontendEventEnvelope<MutsukiFrontendEvent>[] {
  if (envelope.payload.type !== "batch") return [envelope];
  return envelope.payload.events.flatMap((payload) =>
    flattenEnvelope({
      sequence: envelope.sequence,
      channel: eventChannel(payload),
      payload,
    }),
  );
}

function eventChannel(event: MutsukiFrontendEvent): string {
  switch (event.type) {
    case "batch": return "mutsuki://event/batch";
    case "observability_gap": return "mutsuki://observe/gap";
    case "task": return "mutsuki://task/event";
    case "runtime": return "mutsuki://runtime/event";
    case "trace": return "mutsuki://trace/event";
    case "log": return "mutsuki://log/event";
    case "resource": return "mutsuki://resource/event";
    case "approval": return "mutsuki://approval/event";
    case "plugin": return "mutsuki://plugin/event";
    case "runner": return "mutsuki://runner/event";
  }
}
