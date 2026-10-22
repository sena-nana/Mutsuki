import { defineRpc, mutation, query, topic, type JsonValue } from "@mutsuki/web-sdk";

export interface QqSelfUser {
  user_id?: string | null;
  display_name?: string | null;
  avatar_url?: string | null;
}

export interface QqAccount {
  account_id: string;
  enabled: boolean;
  health: string;
  connection_state: string;
  connected_since_unix_ms?: number | null;
  last_error?: string | null;
  self_user?: QqSelfUser | null;
}

export interface QqSnapshot {
  revision: number;
  accounts: QqAccount[];
}

export type DeliveryStatus =
  | "pending"
  | "sending"
  | "succeeded"
  | "retry_scheduled"
  | "permanently_failed"
  | "cancelled"
  | "previewed"
  | "reconcile_required";

export interface QqDelivery {
  receipt: {
    delivery_id: string;
    status: DeliveryStatus;
    attempt_count: number;
    delivered_at_unix_ms?: number | null;
    error_code?: string | null;
  };
  attempts: Array<{ attempt: number; status: DeliveryStatus; started_at_unix_ms: number; error_code?: string | null }>;
}

export type InteractionStatus = "waiting" | "completed" | "timed_out" | "cancelled" | "failed";

export interface QqInteraction {
  session_id: string;
  conversation: { platform: string; account_id: string; kind: string; user_id?: string; group_id?: string; channel_id?: string };
  actor_id?: string | null;
  status: InteractionStatus;
  retries_remaining: number;
  wait: { timeout_at_unix_ms: number; command?: string | null };
}

export interface Page<T> {
  items: T[];
  next_cursor?: string | null;
}

export type QqAction =
  | { action: "delivery_retry"; delivery_id: string }
  | { action: "delivery_cancel"; delivery_id: string }
  | { action: "delivery_preview"; delivery_id: string }
  | { action: "interaction_cancel"; session_id: string };

export interface PageParams {
  query?: string;
  after?: string;
  limit?: number;
}

/**
 * `qq-bot.*` RPCs of `mutsuki-bot-web-extension-qq`. Deliveries and interactions still go through
 * the QQ owner until a platform-neutral delivery namespace exists (plan W7a).
 */
export const qqApi = defineRpc({
  namespace: "qq-bot",
  methods: {
    snapshot: query<{ query?: string }, QqSnapshot>({ capability: "bot.read" }),
    "deliveries.list": query<PageParams, Page<QqDelivery>>({ capability: "bot.read" }),
    "interactions.list": query<PageParams, Page<QqInteraction>>({ capability: "bot.read" }),
    write: mutation<
      { request: { operation_id: string; expected_revision: number; action: QqAction }; confirmed: boolean },
      { revision: number; audit_id: string; result: JsonValue }
    >(),
  },
});

export const qqChanged = topic<{ revision?: number }>("qq.changed", "bot.read");
export const PAGE_SIZE = 50;
