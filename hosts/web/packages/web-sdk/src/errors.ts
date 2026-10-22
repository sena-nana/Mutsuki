import type { JsonValue } from "./json.js";

/** Codes the browser bridge itself produces; host and extension codes pass through as-is. */
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
  | "rpc_failed"
  | "auth_failed";

/**
 * Failures that are safe to retry as-is: the request never reached the host or the host
 * reported a transient condition. `delivery_unknown` is deliberately absent — a write may
 * already have taken effect, so the caller must re-read before retrying.
 */
const RETRYABLE_CODES = new Set([
  "not_connected",
  "pending_limit",
  "hello_timeout",
  "request_timeout",
  "send_failed",
  "disconnected",
  "queue_full",
  "budget_exceeded",
]);

export function isRetryableCode(code: string): boolean {
  return RETRYABLE_CODES.has(code);
}

/** One structured RPC failure. Branch on `code`, never on `message`. */
export class RpcError extends Error {
  readonly code: string;
  readonly details: JsonValue | undefined;
  readonly retryable: boolean;

  constructor(
    code: BridgeErrorCode | string,
    message: string,
    options: { details?: JsonValue; retryable?: boolean; cause?: unknown } = {},
  ) {
    super(message, options.cause === undefined ? undefined : { cause: options.cause });
    this.name = "RpcError";
    this.code = code;
    this.details = options.details;
    this.retryable = options.retryable ?? isRetryableCode(code);
  }
}

/** Bridge-originated failure; kept as a subclass so `instanceof RpcError` covers both. */
export class WebBridgeError extends RpcError {
  constructor(
    code: BridgeErrorCode | string,
    message: string,
    options: { details?: JsonValue; retryable?: boolean; cause?: unknown } = {},
  ) {
    super(code, message, options);
    this.name = "WebBridgeError";
  }
}

export function isRpcError(value: unknown): value is RpcError {
  return value instanceof RpcError;
}

/** Normalizes anything thrown into an [`RpcError`]; unknown throws become `internal`. */
export function toRpcError(value: unknown): RpcError {
  if (value instanceof RpcError) return value;
  if (value instanceof Error) {
    return new RpcError("internal", value.message, { cause: value, retryable: false });
  }
  return new RpcError("internal", String(value), { retryable: false });
}
