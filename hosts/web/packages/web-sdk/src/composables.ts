import {
  computed,
  getCurrentScope,
  onScopeDispose,
  readonly,
  ref,
  shallowRef,
  toValue,
  watch,
  type ComputedRef,
  type MaybeRefOrGetter,
  type Ref,
  type ShallowRef,
} from "vue";

import type { BridgeConnectionState } from "./bridge.js";
import { callRpc, subscribeTopic, type RpcMethodRef, type TopicSpec } from "./contract.js";
import { RpcError, toRpcError } from "./errors.js";
import type { Disposable } from "./extension.js";
import type { JsonValue } from "./json.js";
import { useMutsukiRuntime, type RuntimeClient } from "./runtime.js";

function onDispose(dispose: () => void): void {
  if (getCurrentScope()) onScopeDispose(dispose);
}

function documentHidden(): boolean {
  return typeof document !== "undefined" && document.hidden;
}

function onVisible(listener: () => void): Disposable {
  if (typeof document === "undefined") return { dispose() {} };
  const handler = () => {
    if (!document.hidden) listener();
  };
  document.addEventListener("visibilitychange", handler);
  return { dispose: () => document.removeEventListener("visibilitychange", handler) };
}

/** Identical reads in flight on one client share a single request. */
const inflight = new WeakMap<RuntimeClient, Map<string, Promise<unknown>>>();

function dedupedCall<P, R>(client: RuntimeClient, ref: RpcMethodRef<P, R>, params: P): Promise<R> {
  let calls = inflight.get(client);
  if (!calls) {
    calls = new Map();
    inflight.set(client, calls);
  }
  const key = `${ref.key}:${JSON.stringify(params ?? null)}`;
  const existing = calls.get(key);
  if (existing) return existing as Promise<R>;
  const call = (callRpc as (c: RuntimeClient, r: RpcMethodRef<P, R>, p: P) => Promise<R>)(
    client,
    ref,
    params,
  ).finally(() => calls.delete(key));
  calls.set(key, call);
  return call;
}

export interface UseRpcOptions {
  /** Load on creation (default true). */
  immediate?: boolean;
  /** Topics whose events mark the data stale. */
  refreshOn?: Array<TopicSpec<unknown> | string>;
  /** Debounce for event-triggered refreshes (default 50ms). */
  eventDebounceMs?: number;
  /** Defer refreshes while the document is hidden (default true). */
  pauseWhenHidden?: boolean;
}

export interface RpcQuery<R> {
  data: Readonly<ShallowRef<R | undefined>>;
  error: Readonly<Ref<RpcError | null>>;
  loading: Readonly<Ref<boolean>>;
  /** True once a response (data or error) has been applied. */
  settled: Readonly<Ref<boolean>>;
  refresh(): Promise<void>;
}

/**
 * Reactive read. Deduplicates identical requests, drops responses older than the latest
 * request, refreshes on `refreshOn` events, on reconnect, on invalidation and when the
 * params change, and holds refreshes while the page is hidden.
 */
export function useRpc<P, R>(
  method: RpcMethodRef<P, R>,
  params?: MaybeRefOrGetter<P>,
  options: UseRpcOptions = {},
): RpcQuery<R> {
  const { client, queries } = useMutsukiRuntime();
  const data = shallowRef<R | undefined>();
  const error = ref<RpcError | null>(null);
  const loading = ref(false);
  const settled = ref(false);
  const pauseWhenHidden = options.pauseWhenHidden ?? true;
  const disposables: Disposable[] = [];
  let sequence = 0;
  let stale = false;
  let disposed = false;
  let debounce: ReturnType<typeof setTimeout> | undefined;

  const refresh = async (): Promise<void> => {
    if (disposed) return;
    if (pauseWhenHidden && documentHidden()) {
      stale = true;
      return;
    }
    stale = false;
    const current = ++sequence;
    loading.value = true;
    try {
      const result = await dedupedCall(client, method, toValue(params) as P);
      if (current !== sequence || disposed) return;
      data.value = result;
      error.value = null;
    } catch (failure) {
      if (current !== sequence || disposed) return;
      error.value = toRpcError(failure);
    } finally {
      if (current === sequence && !disposed) {
        loading.value = false;
        settled.value = true;
      }
    }
  };

  const scheduleRefresh = () => {
    clearTimeout(debounce);
    debounce = setTimeout(() => void refresh(), options.eventDebounceMs ?? 50);
  };

  for (const entry of options.refreshOn ?? []) {
    const spec = typeof entry === "string" ? { topic: entry } : entry;
    disposables.push(subscribeTopic(client, spec, scheduleRefresh));
  }
  disposables.push(
    queries.listen((key) => {
      if (key === method.key || key === method.namespace) void refresh();
    }),
  );
  if (client.onStateChange) {
    let wasOpen = client.state === "open";
    disposables.push(
      client.onStateChange((state) => {
        if (state === "open" && !wasOpen) void refresh();
        wasOpen = state === "open";
      }),
    );
  }
  if (pauseWhenHidden) {
    disposables.push(
      onVisible(() => {
        if (stale) void refresh();
      }),
    );
  }
  if (params !== undefined) {
    watch(
      () => JSON.stringify(toValue(params) ?? null),
      () => void refresh(),
    );
  }
  if (options.immediate ?? true) void refresh();

  onDispose(() => {
    disposed = true;
    clearTimeout(debounce);
    for (const disposable of disposables.splice(0)) disposable.dispose();
  });

  return {
    data: readonly(data) as Readonly<ShallowRef<R | undefined>>,
    error: readonly(error) as Readonly<Ref<RpcError | null>>,
    loading: readonly(loading),
    settled: readonly(settled),
    refresh,
  };
}

export interface UseMutationOptions<R> {
  /** Methods or namespaces to refresh after success (and after an unknown delivery). */
  invalidates?: Array<RpcMethodRef<any, any> | string>;
  onSuccess?(result: R): void;
}

export interface RpcMutation<P, R> {
  mutate(...params: P extends void ? [params?: P] : [params: P]): Promise<R>;
  pending: Readonly<Ref<boolean>>;
  error: Readonly<Ref<RpcError | null>>;
}

/**
 * Write with explicit invalidation. A `delivery_unknown` failure means the write may have
 * landed, so the declared reads are refreshed before the error reaches the caller.
 */
export function useMutation<P, R>(
  method: RpcMethodRef<P, R>,
  options: UseMutationOptions<R> = {},
): RpcMutation<P, R> {
  const { client, queries } = useMutsukiRuntime();
  const pending = ref(false);
  const error = ref<RpcError | null>(null);
  const invalidate = () => {
    for (const target of options.invalidates ?? []) {
      queries.invalidate(typeof target === "string" ? target : target.key);
    }
  };
  const mutate = async (...params: unknown[]): Promise<R> => {
    pending.value = true;
    error.value = null;
    try {
      const result = await (callRpc as (c: RuntimeClient, r: RpcMethodRef<P, R>, p: unknown) => Promise<R>)(
        client,
        method,
        params[0],
      );
      invalidate();
      options.onSuccess?.(result);
      return result;
    } catch (failure) {
      const rpcError = toRpcError(failure);
      if (rpcError.code === "delivery_unknown") invalidate();
      error.value = rpcError;
      throw rpcError;
    } finally {
      pending.value = false;
    }
  };
  return {
    mutate: mutate as RpcMutation<P, R>["mutate"],
    pending: readonly(pending),
    error: readonly(error),
  };
}

export interface RpcSubscription {
  /** Set once if the host refuses the subscription. */
  error: Readonly<Ref<RpcError | null>>;
}

/** Event subscription owned by the current component or effect scope. */
export function useSubscription<T = JsonValue>(
  spec: TopicSpec<T> | string,
  handler: (payload: T) => void,
): RpcSubscription {
  const { client } = useMutsukiRuntime();
  const error = ref<RpcError | null>(null);
  const subscription = subscribeTopic(
    client,
    typeof spec === "string" ? { topic: spec } : spec,
    handler,
    (failure) => {
      error.value = failure;
    },
  );
  onDispose(() => subscription.dispose());
  return { error: readonly(error) };
}

export interface Polling {
  running: Readonly<Ref<boolean>>;
  start(): void;
  stop(): void;
}

/**
 * Runs `task` every `intervalMs` after the previous run settles (never overlapping), paused
 * while the page is hidden and resumed immediately when it is shown again.
 */
export function usePolling(
  task: () => void | Promise<void>,
  intervalMs: number,
  options: { immediate?: boolean; pauseWhenHidden?: boolean } = {},
): Polling {
  const running = ref(false);
  const pauseWhenHidden = options.pauseWhenHidden ?? true;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let active = false;
  let inFlight = false;

  const tick = async () => {
    timer = undefined;
    if (!active || inFlight) return;
    if (pauseWhenHidden && documentHidden()) return;
    inFlight = true;
    try {
      await task();
    } catch {
      // A failed tick is retried on the next interval; the task reports its own errors.
    } finally {
      inFlight = false;
      if (active && !(pauseWhenHidden && documentHidden())) {
        timer = setTimeout(() => void tick(), intervalMs);
      }
    }
  };
  const visible = pauseWhenHidden
    ? onVisible(() => {
        if (active && !timer && !inFlight) void tick();
      })
    : { dispose() {} };

  const start = () => {
    if (active) return;
    active = true;
    running.value = true;
    if (options.immediate ?? true) void tick();
    else timer = setTimeout(() => void tick(), intervalMs);
  };
  const stop = () => {
    active = false;
    running.value = false;
    clearTimeout(timer);
    timer = undefined;
  };
  start();
  onDispose(() => {
    stop();
    visible.dispose();
  });
  return { running: readonly(running), start, stop };
}

/** Live bridge connection state. */
export function useConnectionState(): Readonly<Ref<BridgeConnectionState>> {
  const { client } = useMutsukiRuntime();
  const state = ref<BridgeConnectionState>(client.state ?? "open");
  const subscription = client.onStateChange?.((next) => {
    state.value = next;
  });
  onDispose(() => subscription?.dispose());
  return readonly(state);
}

export interface Session {
  connection: Readonly<Ref<BridgeConnectionState>>;
  /** The host refused the credentials; the shell should show its login. */
  unauthorized: ComputedRef<boolean>;
  capabilities: ComputedRef<ReadonlySet<string>>;
  can(capability: string | undefined): boolean;
  login(token: string): Promise<boolean>;
  logout(): Promise<void>;
}

/** Login state and capability checks for UI gating. The host still enforces every call. */
export function useSession(): Session {
  const { session } = useMutsukiRuntime();
  const connection = useConnectionState();
  const capabilities = computed<ReadonlySet<string>>(() => {
    // Re-evaluated on every connection change: a reconnect may carry a new grant.
    void connection.value;
    return session?.capabilities() ?? new Set();
  });
  const unavailable = () =>
    Promise.reject(new RpcError("session_unavailable", "the shell provides no login session"));
  return {
    connection,
    unauthorized: computed(() => connection.value === "unauthorized"),
    capabilities,
    can: (capability) =>
      !capability || capabilities.value.has("*") || capabilities.value.has(capability),
    login: (token) => (session ? session.login(token) : unavailable()),
    logout: () => (session ? session.logout() : unavailable()),
  };
}
