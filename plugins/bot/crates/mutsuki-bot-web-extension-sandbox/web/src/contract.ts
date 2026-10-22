import { defineRpc, mutation, query, topic, type JsonValue } from "@mutsuki/web-sdk";

export type SandboxMode = "simulate" | "live";

export interface SandboxUser {
  user_id: string;
  display_name: string;
  avatar_url?: string | null;
}

export interface Conversation {
  conversation_id: string;
  kind: "group" | "private" | "channel" | string;
  title: string;
  avatar_url?: string | null;
  conversation?: { user_id?: string | null } | null;
  users: SandboxUser[];
  last_preview?: string | null;
  last_activity_unix_ms: number;
  active_message?: boolean;
}

export interface Snapshot {
  revision: number;
  mode: SandboxMode;
  live_available?: boolean;
  conversations: Conversation[];
  live_users?: SandboxUser[];
  bot?: SandboxUser | null;
}

export interface ContentRef {
  t: string;
  at: number;
  h?: string | null;
  id?: string | null;
  name?: string | null;
  mime?: string | null;
  url?: string | null;
  p?: Record<string, unknown> | null;
}

export interface Message {
  message_id: string;
  conversation_id: string;
  sender_id: string;
  sender_name: string;
  role: "user" | "bot" | "system";
  text: string;
  refs?: ContentRef[];
  reply_to?: string | null;
  time_ms: number;
}

export type Segment =
  | { type: "mention_user"; user_id: string }
  | { type: "mention_all" }
  | { type: "markdown"; content: string }
  | { type: "platform_specific"; platform: string; kind: string; payload: Record<string, unknown> };

export type SandboxAction =
  | { action: "set_mode"; mode: SandboxMode }
  | { action: "add_user" }
  | { action: "update_user"; user_id: string; new_user_id: string; display_name: string }
  | { action: "import_live_users"; user_ids: string[] }
  | { action: "remove_user"; user_id: string }
  | { action: "clear_conversation"; conversation_id: string }
  | {
      action: "ingest_as_user" | "send_as_bot";
      conversation_id: string;
      user_id?: string;
      text: string;
      segments: Segment[];
      reply_to: string | null;
    };

export interface Sticker {
  id: string;
  kind: "custom" | "qq_face";
  mime?: string | null;
  name?: string | null;
  face_type?: string | null;
  face_id?: string | null;
}

export interface Blob64 {
  mime: string;
  name: string;
  bytes: string;
}

/** `sandbox.*` RPCs of `mutsuki-bot-web-extension-sandbox`. */
export const sandboxApi = defineRpc({
  namespace: "sandbox",
  methods: {
    snapshot: query<{ query: string }, Snapshot>({ capability: "bot.read" }),
    messages: query<{ conversation_id: string }, Message[]>({ capability: "bot.read" }),
    // `send_as_bot` needs `bot.delivery.write`; the host checks per action.
    write: mutation<
      { confirmed: boolean; request: { operation_id: string; expected_revision: number; action: SandboxAction } },
      { revision: number; result: JsonValue }
    >(),
    "media.upload": mutation<{ name: string; mime: string; bytes: string }, { media_id: string; mime: string; name: string }>({
      capability: "bot.sandbox.write",
    }),
    "media.get": query<{ media_id: string }, Blob64 & { media_id: string }>({ capability: "bot.read" }),
    "sticker.upload": mutation<{ name: string; mime: string; bytes: string }, { sticker_id: string }>({ capability: "bot.sandbox.write" }),
    "sticker.list": query<void, Sticker[]>({ capability: "bot.read" }),
    "sticker.get": query<{ sticker_id: string }, Blob64 & { sticker_id: string }>({ capability: "bot.read" }),
  },
});

export const sandboxChanged = topic<{ revision?: number; mode?: SandboxMode }>("sandbox.changed", "bot.read");
