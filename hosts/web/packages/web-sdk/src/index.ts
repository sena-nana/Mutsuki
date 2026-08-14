/** Controlled Vue extension SDK. No raw token / IPC access. */

import { decode as decodeMsgpack, encode as encodeMsgpack } from "@msgpack/msgpack";

export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };

export interface Disposable {
  dispose(): void;
}

/** Owns reversible extension registrations with Scope-compatible lifecycle semantics. */
export class DisposableScope implements Disposable {
  private effects: Disposable[] = [];
  private closed = false;
  private disposed = false;

  own(effect: Disposable): Disposable {
    if (this.closed) {
      effect.dispose();
      throw new Error("disposable scope is already disposed");
    }
    let active = true;
    const owned = {
      dispose() {
        if (!active) return;
        effect.dispose();
        active = false;
      },
    };
    this.effects.push(owned);
    return owned;
  }

  dispose(): void {
    if (this.disposed) return;
    this.closed = true;
    const failures: unknown[] = [];
    const failedEffects: Disposable[] = [];
    for (const effect of this.effects.splice(0).reverse()) {
      try {
        effect.dispose();
      } catch (error) {
        failures.push(error);
        failedEffects.push(effect);
      }
    }
    this.effects = failedEffects.reverse();
    this.disposed = failures.length === 0;
    if (failures.length > 0) {
      throw new AggregateError(failures, "extension scope disposal failed");
    }
  }
}

export interface PageRegistration {
  id: string;
  path: string;
  title: string;
  component: unknown;
  requiredCapability?: string;
}

export interface NavigationRegistration {
  id: string;
  label: string;
  path: string;
  order?: number;
  requiredCapability?: string;
}

export interface SlotRegistration {
  id: string;
  slot: string;
  component: unknown;
  requiredCapability?: string;
}

export interface CommandRegistration {
  id: string;
  title: string;
  run: () => void | Promise<void>;
  requiredCapability?: string;
}

export interface ExtensionContext {
  pages: Registry<PageRegistration>;
  navigation: Registry<NavigationRegistration>;
  slots: Registry<SlotRegistration>;
  commands: Registry<CommandRegistration>;
  rpc: RpcClient;
  events: EventClient;
}

export interface Registry<T> {
  register(item: T): Disposable;
}

export interface WebExtension {
  id: string;
  setup(ctx: ExtensionContext): void | Disposable | Promise<void | Disposable>;
}

export interface RpcClient {
  call(
    namespace: string,
    method: string,
    params?: JsonValue,
    options?: RpcCallOptions,
  ): Promise<JsonValue>;
  read(namespace: string, method: string, params?: JsonValue): Promise<JsonValue>;
  write(namespace: string, method: string, params?: JsonValue): Promise<JsonValue>;
}

export interface RpcCallOptions {
  operation?: "read" | "write";
  timeoutMs?: number;
}

export interface EventClient {
  subscribe(
    topic: string,
    handler: (payload: JsonValue) => void,
    requiredCapability?: string,
  ): Disposable;
}

export interface BridgeHelloAck {
  protocol_version: string;
  session: {
    session_id: string;
    capabilities: string[];
    safe_mode: boolean;
  };
}

type WireRecord = Record<string, unknown>;

export type BridgeConnectionState =
  | "idle"
  | "connecting"
  | "open"
  | "reconnecting"
  | "closed";

export type BridgeErrorCode =
  | "not_connected"
  | "pending_limit"
  | "hello_timeout"
  | "request_timeout"
  | "send_failed"
  | "disconnected"
  | "delivery_unknown"
  | "closed"
  | "protocol_error"
  | "rpc_failed";

export class WebBridgeError extends Error {
  constructor(
    public readonly code: BridgeErrorCode | string,
    message: string,
  ) {
    super(message);
    this.name = "WebBridgeError";
  }
}

export interface WebBridgeClientOptions {
  authToken?: string;
  capabilities?: string[];
  helloTimeoutMs?: number;
  requestTimeoutMs?: number;
  maxPending?: number;
  reconnectBaseDelayMs?: number;
  reconnectMaxDelayMs?: number;
  webSocketFactory?: (url: string) => WebSocket;
}

interface PendingRpc {
  generation: number;
  operation: "read" | "write";
  sent: boolean;
  timer: ReturnType<typeof setTimeout>;
  resolve: (value: JsonValue) => void;
  reject: (error: Error) => void;
}

interface LogicalSubscription {
  topic: string;
  handler: (payload: JsonValue) => void;
  requiredCapability?: string;
}

/** Browser-side bridge client. Tokens never leave this module. */
export class WebBridgeClient implements RpcClient, EventClient {
  private ws: WebSocket | null = null;
  private sessionId: string | null = null;
  private readonly pending = new Map<string, PendingRpc>();
  private readonly subscriptions = new Map<string, LogicalSubscription>();
  private readonly stateListeners = new Set<(state: BridgeConnectionState) => void>();
  private readonly authToken: string | undefined;
  private readonly capabilities: string[];
  private readonly helloTimeoutMs: number;
  private readonly requestTimeoutMs: number;
  private readonly maxPending: number;
  private readonly reconnectBaseDelayMs: number;
  private readonly reconnectMaxDelayMs: number;
  private readonly webSocketFactory: (url: string) => WebSocket;
  private reconnectAttempt = 0;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private generation = 0;
  private connectionState: BridgeConnectionState = "idle";
  private helloAck: BridgeHelloAck | null = null;
  private connectPromise: Promise<BridgeHelloAck> | null = null;
  private helloReject: ((error: Error) => void) | null = null;
  private helloTimer: ReturnType<typeof setTimeout> | null = null;
  private idSequence = 0;

  constructor(
    private readonly url: string,
    options: WebBridgeClientOptions = {},
  ) {
    this.authToken = options.authToken;
    this.capabilities = [...(options.capabilities ?? [])];
    this.helloTimeoutMs = positive(options.helloTimeoutMs, 5_000);
    this.requestTimeoutMs = positive(options.requestTimeoutMs, 30_000);
    this.maxPending = positive(options.maxPending, 128);
    this.reconnectBaseDelayMs = positive(options.reconnectBaseDelayMs, 1_000);
    this.reconnectMaxDelayMs = positive(options.reconnectMaxDelayMs, 10_000);
    this.webSocketFactory = options.webSocketFactory ?? ((url) => new WebSocket(url));
  }

  async connect(protocolVersion = "1.0.0"): Promise<BridgeHelloAck> {
    if (this.connectionState === "open" && this.helloAck) return this.helloAck;
    if (this.connectPromise) return this.connectPromise;
    if (this.connectionState === "closed") this.setState("idle");
    this.connectPromise = this.openGeneration(protocolVersion);
    return this.connectPromise;
  }

  get state(): BridgeConnectionState {
    return this.connectionState;
  }

  get pendingCount(): number {
    return this.pending.size;
  }

  onStateChange(listener: (state: BridgeConnectionState) => void): Disposable {
    this.stateListeners.add(listener);
    listener(this.connectionState);
    return { dispose: () => this.stateListeners.delete(listener) };
  }

  private openGeneration(protocolVersion: string): Promise<BridgeHelloAck> {
    this.clearReconnectTimer();
    const generation = ++this.generation;
    this.setState("connecting");
    this.sessionId = null;
    this.helloAck = null;
    return new Promise<BridgeHelloAck>((resolve, reject) => {
      this.helloReject = reject;
      let ws: WebSocket;
      try {
        ws = this.webSocketFactory(this.url);
      } catch (error) {
        this.connectPromise = null;
        this.helloReject = null;
        reject(new WebBridgeError("send_failed", asError(error).message));
        this.scheduleReconnect(protocolVersion);
        return;
      }
      ws.binaryType = "arraybuffer";
      this.ws = ws;
      this.helloTimer = setTimeout(() => {
        if (!this.isCurrent(generation, ws) || this.sessionId) return;
        const error = new WebBridgeError("hello_timeout", "web bridge hello timed out");
        this.failHello(generation, error);
        ws.close();
      }, this.helloTimeoutMs);
      ws.addEventListener("open", () => {
        if (!this.isCurrent(generation, ws)) return;
        try {
          ws.send(
            encodeWireMessage({
              type: "hello",
              protocol_version: protocolVersion,
              capabilities: this.capabilities,
              // Token is sent once for auth and never exposed to extensions.
              auth_token: this.authToken,
            }),
          );
        } catch (error) {
          this.failHello(
            generation,
            new WebBridgeError("send_failed", asError(error).message),
          );
          ws.close();
        }
      });
      ws.addEventListener("message", (event) => {
        void decodeWireMessage(event.data)
          .then((message) => {
            if (!this.isCurrent(generation, ws)) return;
            const ack = this.dispatch(generation, message);
            if (ack) {
              this.clearHelloTimer();
              this.helloReject = null;
              this.connectPromise = null;
              this.helloAck = ack;
              this.sessionId = ack.session.session_id;
              this.reconnectAttempt = 0;
              this.setState("open");
              this.restoreSubscriptions(generation);
              resolve(ack);
            }
          })
          .catch((error) => {
            if (!this.isCurrent(generation, ws) || this.sessionId) return;
            this.failHello(
              generation,
              new WebBridgeError("protocol_error", asError(error).message),
            );
            ws.close();
          });
      });
      ws.addEventListener("close", () => {
        if (!this.isCurrent(generation, ws)) return;
        this.handleDisconnect(generation, protocolVersion);
      });
      ws.addEventListener("error", () => {
        if (!this.isCurrent(generation, ws) || this.sessionId) return;
        this.failHello(
          generation,
          new WebBridgeError("disconnected", "websocket failed before hello"),
        );
      });
    });
  }

  private dispatch(generation: number, message: Record<string, unknown>): BridgeHelloAck | null {
    if (message.type === "hello_ack") {
      const ack = message as unknown as BridgeHelloAck;
      return {
        ...ack,
        session: {
          ...ack.session,
          session_id: uuidKey(ack.session.session_id),
        },
      };
    }
    if (message.type === "rpc_result") {
      const id = uuidKey(message.id);
      const pending = this.pending.get(id);
      if (!pending || pending.generation !== generation) return null;
      this.pending.delete(id);
      clearTimeout(pending.timer);
      if (message.error) {
        const error = message.error as { code?: string; message?: string };
        pending.reject(
          new WebBridgeError(error.code ?? "rpc_failed", error.message ?? "rpc failed"),
        );
      } else {
        pending.resolve((message.result ?? null) as JsonValue);
      }
      return null;
    }
    if (message.type === "event") {
      const subscriptionId = uuidKey(message.subscription_id);
      const sub = this.subscriptions.get(subscriptionId);
      sub?.handler((message.payload ?? null) as JsonValue);
    }
    return null;
  }

  async call(
    namespace: string,
    method: string,
    params: JsonValue = null,
    options: RpcCallOptions = {},
  ): Promise<JsonValue> {
    if (this.connectionState !== "open" || !this.ws) {
      throw new WebBridgeError("not_connected", "web bridge is not open");
    }
    if (this.pending.size >= this.maxPending) {
      throw new WebBridgeError("pending_limit", `pending RPC limit ${this.maxPending} exceeded`);
    }
    const generation = this.generation;
    const id = this.nextId();
    const payload = {
      type: "rpc",
      id,
      namespace,
      method,
      params,
    };
    return new Promise((resolve, reject) => {
      const timeoutMs = positive(options.timeoutMs, this.requestTimeoutMs);
      const pending: PendingRpc = {
        generation,
        operation: options.operation ?? "write",
        sent: false,
        timer: setTimeout(() => {
          if (this.pending.get(id) !== pending) return;
          this.pending.delete(id);
          reject(new WebBridgeError("request_timeout", "web bridge request timed out"));
        }, timeoutMs),
        resolve,
        reject,
      };
      this.pending.set(id, pending);
      try {
        this.sendCurrent(generation, payload);
        pending.sent = true;
      } catch (error) {
        this.pending.delete(id);
        clearTimeout(pending.timer);
        reject(new WebBridgeError("send_failed", asError(error).message));
      }
    });
  }

  read(namespace: string, method: string, params: JsonValue = null): Promise<JsonValue> {
    return this.call(namespace, method, params, { operation: "read" });
  }

  write(namespace: string, method: string, params: JsonValue = null): Promise<JsonValue> {
    return this.call(namespace, method, params, { operation: "write" });
  }

  subscribe(
    topic: string,
    handler: (payload: JsonValue) => void,
    requiredCapability?: string,
  ): Disposable {
    const subscriptionId = this.nextId();
    this.subscriptions.set(subscriptionId, { topic, handler, requiredCapability });
    if (this.connectionState === "open") this.sendSubscription(subscriptionId);
    return {
      dispose: () => {
        if (!this.subscriptions.delete(subscriptionId)) return;
        if (this.connectionState === "open") {
          try {
            this.sendCurrent(this.generation, {
              type: "unsubscribe",
              subscription_id: subscriptionId,
            });
          } catch {
            // The logical subscription is already disposed; reconnect must not restore it.
          }
        }
      },
    };
  }

  close(): void {
    if (this.connectionState === "closed") return;
    this.setState("closed");
    this.clearReconnectTimer();
    this.clearHelloTimer();
    this.helloReject?.(new WebBridgeError("closed", "web bridge closed"));
    this.helloReject = null;
    this.connectPromise = null;
    this.subscriptions.clear();
    this.rejectGeneration(this.generation, "closed");
    const ws = this.ws;
    this.ws?.close();
    this.ws = null;
    this.sessionId = null;
    this.helloAck = null;
    // Any queued message from the retired socket is now stale.
    this.generation += 1;
    void ws;
  }

  private handleDisconnect(generation: number, protocolVersion: string): void {
    if (generation !== this.generation || this.connectionState === "closed") return;
    this.clearHelloTimer();
    this.failHello(
      generation,
      new WebBridgeError("disconnected", "web bridge disconnected before hello"),
    );
    this.rejectGeneration(generation, "disconnected");
    this.ws = null;
    this.sessionId = null;
    this.helloAck = null;
    this.scheduleReconnect(protocolVersion);
  }

  private scheduleReconnect(protocolVersion: string): void {
    if (this.connectionState === "closed" || this.reconnectTimer) return;
    this.setState("reconnecting");
    this.reconnectAttempt += 1;
    const delay = Math.min(
      this.reconnectBaseDelayMs * 2 ** Math.max(0, this.reconnectAttempt - 1),
      this.reconnectMaxDelayMs,
    );
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      if (this.connectionState === "closed") return;
      this.connectPromise = this.openGeneration(protocolVersion);
      void this.connectPromise.catch(() => undefined);
    }, delay);
  }

  private failHello(generation: number, error: Error): void {
    if (generation !== this.generation || !this.helloReject) return;
    const reject = this.helloReject;
    this.helloReject = null;
    this.connectPromise = null;
    this.clearHelloTimer();
    reject(error);
  }

  private rejectGeneration(generation: number, reason: "disconnected" | "closed"): void {
    for (const [id, pending] of this.pending) {
      if (pending.generation !== generation) continue;
      this.pending.delete(id);
      clearTimeout(pending.timer);
      const code =
        reason === "closed"
          ? "closed"
          : pending.sent && pending.operation === "write"
            ? "delivery_unknown"
            : "disconnected";
      pending.reject(
        new WebBridgeError(
          code,
          code === "delivery_unknown"
            ? "write may have been delivered before disconnect"
            : `web bridge ${reason}`,
        ),
      );
    }
  }

  private restoreSubscriptions(generation: number): void {
    for (const subscriptionId of this.subscriptions.keys()) {
      if (generation !== this.generation) return;
      this.sendSubscription(subscriptionId);
    }
  }

  private sendSubscription(subscriptionId: string): void {
    const subscription = this.subscriptions.get(subscriptionId);
    if (!subscription) return;
    this.sendCurrent(this.generation, {
      type: "subscribe",
      subscription_id: subscriptionId,
      topic: subscription.topic,
      required_capability: subscription.requiredCapability,
    });
  }

  private sendCurrent(generation: number, payload: WireRecord): void {
    if (generation !== this.generation || this.connectionState !== "open" || !this.ws) {
      throw new WebBridgeError("not_connected", "web bridge generation is not open");
    }
    this.ws.send(encodeWireMessage(payload));
  }

  private isCurrent(generation: number, ws: WebSocket): boolean {
    return generation === this.generation && ws === this.ws;
  }

  private clearHelloTimer(): void {
    if (this.helloTimer) clearTimeout(this.helloTimer);
    this.helloTimer = null;
  }

  private clearReconnectTimer(): void {
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.reconnectTimer = null;
  }

  private setState(state: BridgeConnectionState): void {
    if (state === this.connectionState) return;
    this.connectionState = state;
    for (const listener of this.stateListeners) listener(state);
  }

  private nextId(): string {
    this.idSequence += 1;
    return globalThis.crypto?.randomUUID?.() ?? fallbackUuid();
  }
}

function positive(value: number | undefined, fallback: number): number {
  return value != null && Number.isFinite(value) && value > 0 ? value : fallback;
}

function encodeWireMessage(payload: WireRecord): Uint8Array {
  let wire = payload;
  if (payload.type === "rpc") {
    wire = { ...payload, id: uuidBytes(payload.id) };
  } else if (payload.type === "subscribe" || payload.type === "unsubscribe") {
    wire = { ...payload, subscription_id: uuidBytes(payload.subscription_id) };
  }
  return encodeMsgpack(wire);
}

function uuidBytes(value: unknown): Uint8Array {
  if (value instanceof Uint8Array && value.length === 16) return value;
  const compact = uuidKey(value).replaceAll("-", "");
  if (!/^[0-9a-f]{32}$/i.test(compact)) {
    throw new WebBridgeError("protocol_error", "wire UUID is invalid");
  }
  return Uint8Array.from(compact.match(/.{2}/g) ?? [], (byte) => Number.parseInt(byte, 16));
}

function uuidKey(value: unknown): string {
  if (!(value instanceof Uint8Array)) return String(value);
  if (value.length !== 16) {
    throw new WebBridgeError("protocol_error", "wire UUID must contain 16 bytes");
  }
  const hex = Array.from(value, (byte) => byte.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

function fallbackUuid(): string {
  const bytes = new Uint8Array(16);
  for (let index = 0; index < bytes.length; index += 1) {
    bytes[index] = Math.floor(Math.random() * 256);
  }
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  return uuidKey(bytes);
}

async function decodeWireMessage(data: unknown): Promise<WireRecord> {
  let bytes: Uint8Array;
  if (data instanceof ArrayBuffer) {
    bytes = new Uint8Array(data);
  } else if (ArrayBuffer.isView(data)) {
    bytes = new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  } else if (typeof Blob !== "undefined" && data instanceof Blob) {
    bytes = new Uint8Array(await data.arrayBuffer());
  } else {
    throw new Error("web bridge requires binary frames");
  }
  const decoded = decodeMsgpack(bytes);
  if (decoded && typeof decoded === "object" && !Array.isArray(decoded)) {
    return decoded as WireRecord;
  }
  throw new Error("invalid web bridge message");
}

function asError(error: unknown): Error {
  return error instanceof Error ? error : new Error(String(error));
}

export function createRegistry<T extends { id: string }>(): Registry<T> & {
  list(): T[];
  clear(): void;
} {
  const items = new Map<string, T>();
  return {
    register(item) {
      items.set(item.id, item);
      return {
        dispose() {
          items.delete(item.id);
        },
      };
    },
    list() {
      return [...items.values()];
    },
    clear() {
      items.clear();
    },
  };
}

/** Error boundary helper — one extension must not crash the shell. */
export function withExtensionBoundary(
  extensionId: string,
  run: () => void,
  onError: (extensionId: string, error: unknown) => void,
): void {
  try {
    run();
  } catch (error) {
    onError(extensionId, error);
  }
}
