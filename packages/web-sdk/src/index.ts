/** Controlled Vue extension SDK. No raw token / IPC access. */

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
  call(namespace: string, method: string, params?: JsonValue): Promise<JsonValue>;
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

/** Browser-side bridge client. Tokens never leave this module. */
export class WebBridgeClient implements RpcClient, EventClient {
  private ws: WebSocket | null = null;
  private sessionId: string | null = null;
  private readonly pending = new Map<
    string,
    { resolve: (value: JsonValue) => void; reject: (error: Error) => void }
  >();
  private readonly subscriptions = new Map<
    string,
    { topic: string; handler: (payload: JsonValue) => void }
  >();
  private readonly authToken: string | undefined;
  private reconnectAttempt = 0;
  private closed = false;

  constructor(
    private readonly url: string,
    options?: { authToken?: string },
  ) {
    this.authToken = options?.authToken;
  }

  async connect(protocolVersion = "1.0.0"): Promise<BridgeHelloAck> {
    this.closed = false;
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(this.url);
      this.ws = ws;
      ws.addEventListener("open", () => {
        ws.send(
          JSON.stringify({
            type: "hello",
            protocol_version: protocolVersion,
            capabilities: [],
            // Token is sent once for auth and never exposed to extensions.
            auth_token: this.authToken,
          }),
        );
      });
      ws.addEventListener("message", (event) => {
        const message = JSON.parse(String(event.data)) as Record<string, unknown>;
        this.dispatch(message, resolve, reject);
      });
      ws.addEventListener("close", () => {
        if (!this.closed) {
          this.scheduleReconnect();
        }
      });
      ws.addEventListener("error", () => reject(new Error("websocket error")));
    });
  }

  private dispatch(
    message: Record<string, unknown>,
    helloResolve?: (value: BridgeHelloAck) => void,
    helloReject?: (error: Error) => void,
  ): void {
    if (message.type === "hello_ack") {
      const ack = message as unknown as BridgeHelloAck;
      this.sessionId = ack.session.session_id;
      this.reconnectAttempt = 0;
      helloResolve?.(ack);
      return;
    }
    if (message.type === "rpc_result") {
      const id = String(message.id);
      const pending = this.pending.get(id);
      if (!pending) return;
      this.pending.delete(id);
      if (message.error) {
        const error = message.error as { message?: string };
        pending.reject(new Error(error.message ?? "rpc failed"));
      } else {
        pending.resolve((message.result ?? null) as JsonValue);
      }
      return;
    }
    if (message.type === "event") {
      const subscriptionId = String(message.subscription_id);
      const sub = this.subscriptions.get(subscriptionId);
      sub?.handler((message.payload ?? null) as JsonValue);
    }
  }

  async call(namespace: string, method: string, params: JsonValue = null): Promise<JsonValue> {
    const id = crypto.randomUUID();
    const payload = {
      type: "rpc",
      id,
      namespace,
      method,
      params,
    };
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.ws?.send(JSON.stringify(payload));
    });
  }

  subscribe(
    topic: string,
    handler: (payload: JsonValue) => void,
    requiredCapability?: string,
  ): Disposable {
    const subscriptionId = crypto.randomUUID();
    this.subscriptions.set(subscriptionId, { topic, handler });
    this.ws?.send(
      JSON.stringify({
        type: "subscribe",
        subscription_id: subscriptionId,
        topic,
        required_capability: requiredCapability,
      }),
    );
    return {
      dispose: () => {
        this.subscriptions.delete(subscriptionId);
        this.ws?.send(
          JSON.stringify({
            type: "unsubscribe",
            subscription_id: subscriptionId,
          }),
        );
      },
    };
  }

  close(): void {
    this.closed = true;
    for (const sub of this.subscriptions.values()) {
      void sub;
    }
    this.subscriptions.clear();
    this.ws?.close();
    this.ws = null;
    this.sessionId = null;
  }

  private scheduleReconnect(): void {
    this.reconnectAttempt += 1;
    const delay = Math.min(1000 * 2 ** this.reconnectAttempt, 10_000);
    setTimeout(() => {
      if (!this.closed) {
        void this.connect();
      }
    }, delay);
  }
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
