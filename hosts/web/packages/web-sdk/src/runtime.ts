import { inject, type App, type InjectionKey } from "vue";

import type { BridgeConnectionState, EventClient, RpcClient } from "./bridge.js";
import type { Disposable } from "./extension.js";
import { RpcError } from "./errors.js";

/** The bridge as extensions see it: RPC, events and connection state, never the socket. */
export interface RuntimeClient extends RpcClient, EventClient {
  readonly state?: BridgeConnectionState;
  onStateChange?(listener: (state: BridgeConnectionState) => void): Disposable;
}

/** Browser login backed by the host's `/auth/session` cookie. */
export interface SessionApi {
  /** Server-granted capabilities of the current connection. */
  capabilities(): ReadonlySet<string>;
  /** Exchanges a token for the session cookie; resolves false when the token is rejected. */
  login(token: string): Promise<boolean>;
  logout(): Promise<void>;
}

/** What the shell provides to every Vue extension. */
export interface MutsukiRuntime {
  client: RuntimeClient;
  session?: SessionApi;
  /** Shared query invalidation; created on demand when the shell provides none. */
  queries?: QueryBus;
}

/**
 * Invalidation channel between `useMutation` and live `useRpc` queries. Keys are
 * `namespace.method`; invalidating a namespace refreshes every method in it.
 */
export class QueryBus {
  private readonly listeners = new Set<(key: string) => void>();

  listen(listener: (key: string) => void): Disposable {
    this.listeners.add(listener);
    return { dispose: () => this.listeners.delete(listener) };
  }

  invalidate(key: string): void {
    for (const listener of [...this.listeners]) listener(key);
  }
}

export const MUTSUKI_RUNTIME: InjectionKey<MutsukiRuntime> = Symbol("mutsuki.runtime");

const fallbackBuses = new WeakMap<RuntimeClient, QueryBus>();

/** Installs the runtime for every component below `app`. */
export function provideMutsukiRuntime(app: App, runtime: MutsukiRuntime): void {
  app.provide(MUTSUKI_RUNTIME, runtime);
}

/** The runtime provided by the shell; a component outside any shell fails loudly. */
export function useMutsukiRuntime(): MutsukiRuntime & { queries: QueryBus } {
  const runtime = inject(MUTSUKI_RUNTIME, null);
  if (!runtime) {
    throw new RpcError(
      "runtime_missing",
      "no Mutsuki runtime provided; mount the component through the shell or provideMutsukiRuntime",
      { retryable: false },
    );
  }
  let queries = runtime.queries;
  if (!queries) {
    queries = fallbackBuses.get(runtime.client) ?? new QueryBus();
    fallbackBuses.set(runtime.client, queries);
  }
  return { ...runtime, queries };
}

/** `/auth/session` login against the document's own origin. */
export function createCookieSession(
  capabilities: () => ReadonlySet<string>,
  fetchImpl: typeof fetch = globalThis.fetch.bind(globalThis),
): SessionApi {
  return {
    capabilities,
    async login(token) {
      const response = await fetchImpl("./auth/session", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ token }),
        credentials: "same-origin",
      });
      if (response.status === 401) return false;
      if (!response.ok) {
        throw new RpcError("login_failed", `login failed with HTTP ${response.status}`);
      }
      return true;
    },
    async logout() {
      await fetchImpl("./auth/logout", { method: "POST", credentials: "same-origin" });
    },
  };
}
