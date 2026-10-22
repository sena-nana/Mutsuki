/**
 * Test doubles for extension authors: `@mutsuki/web-sdk/testing`. Never shipped in the
 * shared runtime.
 */
import { createApp, defineComponent, h, type App } from "vue";

import type { BridgeConnectionState, RpcCallOptions } from "./bridge.js";
import { RpcError } from "./errors.js";
import { createRegistry, type Disposable, type ExtensionContext } from "./extension.js";
import { createI18n, provideI18n, type I18n } from "./i18n.js";
import type { JsonValue } from "./json.js";
import { QueryBus, provideMutsukiRuntime, type RuntimeClient, type SessionApi } from "./runtime.js";

/** Returns whatever the RPC would; plain object literals are fine (no JSON typing needed). */
export type FakeHandler = (params: JsonValue) => unknown;

export interface FakeCall {
  namespace: string;
  method: string;
  params: JsonValue;
  operation: "read" | "write";
}

export interface FakeBridge extends RuntimeClient {
  readonly calls: FakeCall[];
  /** Replaces or adds a handler for `namespace.method`. */
  handle(key: string, handler: FakeHandler): void;
  /** Delivers an event to every live subscription of `topic`. */
  emit(topic: string, payload: JsonValue): void;
  /** Refuses future subscriptions to `topic` with `capability_denied`. */
  deny(topic: string): void;
  setState(state: BridgeConnectionState): void;
  /** Live subscription count per topic. */
  subscriptions(topic: string): number;
}

export interface FakeBridgeOptions {
  handlers?: Record<string, FakeHandler>;
  state?: BridgeConnectionState;
}

/** In-memory bridge: handlers keyed `namespace.method`; unknown methods fail `method_not_found`. */
export function createFakeBridge(options: FakeBridgeOptions = {}): FakeBridge {
  const handlers = new Map(Object.entries(options.handlers ?? {}));
  const calls: FakeCall[] = [];
  const listeners = new Set<(state: BridgeConnectionState) => void>();
  const subscribers = new Map<string, Set<(payload: JsonValue) => void>>();
  const denied = new Set<string>();
  let state: BridgeConnectionState = options.state ?? "open";

  const call = async (
    namespace: string,
    method: string,
    params: JsonValue = null,
    callOptions: RpcCallOptions = {},
  ): Promise<JsonValue> => {
    calls.push({ namespace, method, params, operation: callOptions.operation ?? "write" });
    if (state !== "open") throw new RpcError("not_connected", "fake bridge is not open");
    const handler = handlers.get(`${namespace}.${method}`);
    if (!handler) {
      throw new RpcError("method_not_found", `no fake handler for ${namespace}.${method}`, {
        retryable: false,
      });
    }
    return (await handler(params)) as JsonValue;
  };

  return {
    calls,
    get state() {
      return state;
    },
    call,
    read: (namespace, method, params) => call(namespace, method, params, { operation: "read" }),
    write: (namespace, method, params) => call(namespace, method, params, { operation: "write" }),
    subscribe(topic, handler, _capability, onError): Disposable {
      if (denied.has(topic)) {
        queueMicrotask(() =>
          onError?.(new RpcError("capability_denied", `subscription to ${topic} denied`)),
        );
        return { dispose() {} };
      }
      const set = subscribers.get(topic) ?? new Set();
      set.add(handler);
      subscribers.set(topic, set);
      return { dispose: () => set.delete(handler) };
    },
    onStateChange(listener) {
      listeners.add(listener);
      listener(state);
      return { dispose: () => listeners.delete(listener) };
    },
    handle(key, handler) {
      handlers.set(key, handler);
    },
    emit(topic, payload) {
      for (const handler of [...(subscribers.get(topic) ?? [])]) handler(payload);
    },
    deny(topic) {
      denied.add(topic);
    },
    setState(next) {
      if (next === state) return;
      state = next;
      for (const listener of [...listeners]) listener(next);
    },
    subscriptions: (topic) => subscribers.get(topic)?.size ?? 0,
  };
}

/** A shell-like extension context over a fake bridge, with real registries. */
export function createTestExtensionContext(bridge: FakeBridge = createFakeBridge()): ExtensionContext {
  return {
    pages: createRegistry(),
    navigation: createRegistry(),
    slots: createRegistry(),
    commands: createRegistry(),
    rpc: bridge,
    events: bridge,
  };
}

export interface MountedComposable<T> {
  result: T;
  app: App;
  unmount(): void;
}

/**
 * Runs `setup` inside a mounted component with the runtime (and optionally i18n/session)
 * provided, so composables behave exactly as under the shell.
 */
export function withRuntime<T>(
  setup: () => T,
  options: { bridge?: FakeBridge; session?: SessionApi; i18n?: I18n; queries?: QueryBus } = {},
): MountedComposable<T> {
  let result!: T;
  const app = createApp(
    defineComponent({
      setup() {
        result = setup();
        return () => h("div");
      },
    }),
  );
  provideMutsukiRuntime(app, {
    client: options.bridge ?? createFakeBridge(),
    session: options.session,
    queries: options.queries,
  });
  provideI18n(app, options.i18n ?? createI18n());
  const host = document.createElement("div");
  app.mount(host);
  return { result, app, unmount: () => app.unmount() };
}
