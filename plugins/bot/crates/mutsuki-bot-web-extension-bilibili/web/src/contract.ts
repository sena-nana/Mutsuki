import { defineRpc, mutation, query, topic } from "@mutsuki/web-sdk";

export type NotificationKind = "live" | "dynamic" | "video";
export const NOTIFICATION_KINDS: readonly NotificationKind[] = ["live", "dynamic", "video"];

export type BotTarget =
  | { type: "group"; group_id: string }
  | { type: "user"; user_id: string }
  | { type: "guild_channel"; guild_id: string; channel_id: string }
  | { type: "conversation"; conversation_id: string }
  | { type: "platform_specific"; platform: string; kind: string; id: string };

export interface BilibiliStatus {
  backend: string;
  management_enabled: boolean;
  allow_self_binding: boolean;
  credential_loaded: boolean;
  subscription_count: number;
  reason?: string | null;
}

export interface Subscription {
  subscription_id: string;
  uid: number;
  notifications: NotificationKind[];
  target: BotTarget;
  outbound_binding: string;
  paused: boolean;
  owner_user_id?: string | null;
}

export interface PreviewCard {
  title: string;
  url: string;
  description: string;
  image_url?: string | null;
}

export type LoginStatus = "pending" | "scanned" | "expired" | "confirmed";

export type BindVerifyResult = { result: "verified" } | { result: "signature_mismatch"; code: string };

/** `bilibili.*` RPCs of `mutsuki-bot-web-extension-bilibili`. */
export const bilibiliApi = defineRpc({
  namespace: "bilibili",
  methods: {
    status: query<void, BilibiliStatus>({ capability: "runtime.read" }),
    "subscriptions.list": query<void, { subscriptions: Subscription[] }>({ capability: "runtime.read" }),
    "subscriptions.preview": query<{ selector: string }, PreviewCard>({ capability: "runtime.read" }),
    "subscriptions.subscribe": mutation<
      {
        subscription_id: string;
        uid: number;
        outbound_binding: string;
        notifications: NotificationKind[];
        target: BotTarget;
      },
      Subscription
    >({ capability: "runtime.write" }),
    "subscriptions.set_paused": mutation<{ selector: string; paused: boolean }, Subscription>({
      capability: "runtime.write",
    }),
    "subscriptions.unsubscribe": mutation<{ subscription_id: string; confirmed: true }, { ok: boolean }>({
      capability: "runtime.write",
    }),
    "login.start": mutation<void, { qr_png_base64: string }>({ capability: "runtime.write" }),
    // Polling a confirmed session stores the new credential, so it is a write.
    "login.poll": mutation<void, { status: LoginStatus; message: string }>({
      capability: "runtime.write",
    }),
    "binding.start": mutation<{ operator_user_id: string; uid: number }, { uid: number; name: string; code: string }>({
      capability: "runtime.write",
    }),
    "binding.verify": mutation<{ operator_user_id: string; platform: string; target: BotTarget }, BindVerifyResult>({
      capability: "runtime.write",
    }),
    "binding.unbind": mutation<{ operator_user_id: string }, { removed: boolean }>({ capability: "runtime.write" }),
  },
});

export const bilibiliChanged = topic<{ revision?: number }>("bilibili.changed", "runtime.read");
