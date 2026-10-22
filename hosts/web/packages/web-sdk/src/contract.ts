import type { Disposable } from "./extension.js";
import type { EventClient, RpcClient } from "./bridge.js";
import { RpcError, toRpcError } from "./errors.js";
import type { JsonValue } from "./json.js";

/**
 * One RPC method of a namespace, typed by its params `P` and result `R`. Until the host
 * generates contracts (B7), owners write these by hand next to their pages.
 */
export interface RpcMethodSpec<P = void, R = unknown> {
  readonly operation: "read" | "write";
  /** Capability the host enforces; the UI uses it only to hide what would be refused. */
  readonly capability?: string;
  readonly timeoutMs?: number;
  /** Type carriers only; never set at runtime. */
  readonly __params?: P;
  readonly __result?: R;
}

/** A read: safe to repeat, deduplicated and cached by `useRpc`. */
export function query<P = void, R = unknown>(
  spec: { capability?: string; timeoutMs?: number } = {},
): RpcMethodSpec<P, R> {
  return { operation: "read", ...spec };
}

/** A write: never repeated implicitly; an unknown delivery invalidates its reads. */
export function mutation<P = void, R = unknown>(
  spec: { capability?: string; timeoutMs?: number } = {},
): RpcMethodSpec<P, R> {
  return { operation: "write", ...spec };
}

/** A typed event topic. */
export interface TopicSpec<T = JsonValue> {
  readonly topic: string;
  readonly capability?: string;
  readonly __payload?: T;
}

export function topic<T = JsonValue>(name: string, capability?: string): TopicSpec<T> {
  return { topic: name, capability };
}

export interface RpcContract {
  namespace: string;
  methods: Record<string, RpcMethodSpec<any, any>>;
}

/** A bound method reference: everything needed to call it, plus its types. */
export interface RpcMethodRef<P = void, R = unknown> extends RpcMethodSpec<P, R> {
  readonly namespace: string;
  readonly method: string;
  /** `namespace.method`, the invalidation key. */
  readonly key: string;
}

export type ParamsOf<M> = M extends RpcMethodSpec<infer P, any> ? P : never;
export type ResultOf<M> = M extends RpcMethodSpec<any, infer R> ? R : never;

export type RpcApi<C extends RpcContract> = {
  readonly [K in keyof C["methods"] & string]: RpcMethodRef<
    ParamsOf<C["methods"][K]>,
    ResultOf<C["methods"][K]>
  >;
} & { readonly $namespace: C["namespace"] };

/** Turns a hand-written contract into method references for `callRpc`/`useRpc`/`useMutation`. */
export function defineRpc<C extends RpcContract>(contract: C): RpcApi<C> {
  if (!/^[a-z][a-z0-9_.-]*$/i.test(contract.namespace)) {
    throw new RpcError("invalid_contract", `invalid RPC namespace: ${contract.namespace}`);
  }
  const api: Record<string, unknown> = { $namespace: contract.namespace };
  for (const [method, spec] of Object.entries(contract.methods)) {
    api[method] = Object.freeze({
      ...spec,
      namespace: contract.namespace,
      method,
      key: `${contract.namespace}.${method}`,
    });
  }
  return Object.freeze(api) as RpcApi<C>;
}

/** Calls one method through any RPC client, normalizing failures to [`RpcError`]. */
export async function callRpc<P, R>(
  client: RpcClient,
  ref: RpcMethodRef<P, R>,
  ...params: P extends void ? [params?: P] : [params: P]
): Promise<R> {
  try {
    const result = await client.call(
      ref.namespace,
      ref.method,
      (params[0] ?? null) as JsonValue,
      { operation: ref.operation, timeoutMs: ref.timeoutMs },
    );
    return result as R;
  } catch (error) {
    throw toRpcError(error);
  }
}

/** Subscribes to a typed topic; denial is reported through `onError` once. */
export function subscribeTopic<T>(
  events: EventClient,
  spec: TopicSpec<T>,
  handler: (payload: T) => void,
  onError?: (error: RpcError) => void,
): Disposable {
  return events.subscribe(
    spec.topic,
    (payload) => handler(payload as T),
    spec.capability,
    onError,
  );
}
