import type { ContentRef, Conversation, Message, SandboxUser, Segment, Snapshot } from "./contract";

/** One piece of a rendered message, in reading order. */
export type Part =
  | { kind: "text"; text: string }
  | { kind: "mention"; text: string }
  | { kind: "media"; mime: string; url: string; mediaId: string; name: string }
  | { kind: "sticker"; stickerId: string; name: string }
  | { kind: "file"; name: string }
  | { kind: "card"; title: string; desc: string; image: string; url: string }
  | { kind: "markdown"; content: string }
  | { kind: "keyboard"; labels: string[] }
  | { kind: "unknown"; label: string };

const IMAGE_PLACEHOLDER = "[图片]";

export const kindLabel = (kind: string | undefined) =>
  ({ group: "群聊", private: "私聊", channel: "频道" })[kind ?? ""] ?? (kind || "会话");

/**
 * QQ CDN images are served over https too; upgrade them so mixed content is never requested.
 * Anything else that is not https is dropped (the CSP would block it anyway).
 */
export function imageUrl(raw: string | null | undefined): string {
  const value = String(raw ?? "").trim();
  if (!value) return "";
  const href = value.startsWith("//") ? `https:${value}` : value.replace(/^http:\/\//i, "https://");
  try {
    const url = new URL(href);
    return url.protocol === "https:" ? url.href : "";
  } catch {
    return "";
  }
}

const stripImagePlaceholder = (text: string, ref: ContentRef | undefined) =>
  ref?.t === "img" && text.endsWith(IMAGE_PLACEHOLDER) ? text.slice(0, -IMAGE_PLACEHOLDER.length) : text;

export function quotedText(messages: Message[], replyTo: string): string {
  const quoted = messages.find((item) => item.message_id === replyTo);
  if (!quoted) return "引用消息";
  const img = (quoted.refs ?? []).find((ref) => ref.t === "img");
  const text = stripImagePlaceholder(quoted.text || "", img);
  return `${quoted.sender_name}: ${text || (img ? "[图片]" : "消息")}`;
}

export function cardFields(payload: Record<string, unknown>) {
  const kv = (payload.kv ?? (payload.ark as { kv?: unknown } | undefined)?.kv ?? []) as Array<{ key?: string; value?: string }>;
  const map = Object.fromEntries((Array.isArray(kv) ? kv : []).map((item) => [item?.key, item?.value ?? ""]));
  const str = (value: unknown) => (typeof value === "string" ? value : "");
  return {
    title: map["#METATITLE#"] || map["#PROMPT#"] || str(payload.title) || "小卡片",
    desc: map["#METADESC#"] || str(payload.description),
    image: map["#PIC#"] || map["#METAIMAGE#"] || str(payload.image),
    url: map["#METAURL#"] || str(payload.url),
  };
}

function refPart(ref: ContentRef): Part {
  const payload = ref.p ?? {};
  switch (ref.t) {
    case "emoji":
      return { kind: "file", name: "表情" };
    case "sticker":
      return ref.h ? { kind: "sticker", stickerId: ref.h, name: ref.name ?? "表情包" } : { kind: "file", name: ref.name ?? "表情包" };
    case "img":
    case "audio":
    case "video":
    case "file": {
      const mime = ref.mime ?? (ref.t === "img" ? "image/*" : "");
      const media = mime.startsWith("image/") || mime.startsWith("audio/") || mime.startsWith("video/") || ref.t === "img";
      return media
        ? { kind: "media", mime, url: ref.url ?? "", mediaId: ref.h ?? "", name: ref.name ?? "图片" }
        : { kind: "file", name: ref.name ?? "文件" };
    }
    case "ark":
    case "embed":
      return { kind: "card", ...cardFields(payload) };
    case "markdown": {
      const content = (payload.content ?? (payload.markdown as { content?: unknown } | undefined)?.content) as string | undefined;
      return { kind: "markdown", content: typeof content === "string" ? content : JSON.stringify(payload) };
    }
    case "keyboard": {
      const rows = ((payload.content as { rows?: unknown } | undefined)?.rows ?? payload.rows ?? []) as Array<{
        buttons?: Array<{ render_data?: { label?: string }; label?: string }>;
      }>;
      const labels = rows.flatMap((row) => row.buttons ?? []).map((item) => item.render_data?.label || item.label || "按钮");
      return { kind: "keyboard", labels: labels.length ? labels : ["按钮"] };
    }
    default:
      return { kind: "unknown", label: `[${ref.t}]` };
  }
}

/** Splits the text at each ref's character offset; mentions keep their `@name` text. */
export function messageParts(message: Message): Part[] {
  const chars = [...(message.text || "")];
  const refs = [...(message.refs ?? [])].sort((a, b) => (a.at || 0) - (b.at || 0));
  const parts: Part[] = [];
  let cursor = 0;
  for (const ref of refs) {
    const at = Math.min(ref.at || 0, chars.length);
    if (at > cursor) {
      const plain = stripImagePlaceholder(chars.slice(cursor, at).join(""), ref);
      if (plain) parts.push({ kind: "text", text: plain });
    }
    if (ref.t === "mention" || ref.t === "mention_all") {
      const length = ref.t === "mention_all" ? [..."@全体成员"].length : 1 + [...(ref.name ?? "")].length;
      parts.push({ kind: "mention", text: chars.slice(at, at + length).join("") || `@${ref.name ?? ref.id ?? ""}` });
      cursor = at + length;
      continue;
    }
    cursor = at;
    parts.push(refPart(ref));
  }
  if (cursor < chars.length) {
    const rest = chars.slice(cursor).join("");
    if (!(rest === IMAGE_PLACEHOLDER && refs.some((ref) => ref.t === "img"))) parts.push({ kind: "text", text: rest });
  }
  if (!parts.length && message.text) parts.push({ kind: "text", text: message.text });
  return parts;
}

/** Consecutive messages from the same sender share one row. */
export function groupMessages(messages: Message[]): Message[][] {
  const groups: Message[][] = [];
  for (const message of messages) {
    const last = groups.at(-1);
    const prev = last?.[0];
    if (prev && prev.role !== "system" && prev.role === message.role && prev.sender_id === message.sender_id) last!.push(message);
    else groups.push([message]);
  }
  return groups;
}

/** In simulate mode "out" is the chosen identity; live mode shows the bot's side as out. */
export function rowLayout(message: Message, identityId: string, live: boolean): "in" | "out" | "system" {
  if (message.role === "system") return "system";
  if (live) return message.role === "bot" ? "out" : "in";
  return message.sender_id === identityId ? "out" : "in";
}

export function canQuote(message: Message, identityId: string, live: boolean): boolean {
  if (message.role === "system") return false;
  return live ? message.role === "user" : message.sender_id !== identityId;
}

export const botProfile = (snapshot: Snapshot | null | undefined): SandboxUser =>
  snapshot?.bot ?? { user_id: "bot", display_name: "机器人" };

/** Members shown beside a conversation; in simulate mode the bot is one of them. */
export function memberRoster(conversation: Conversation | null, snapshot: Snapshot | null, simulate: boolean): SandboxUser[] {
  const users = [...(conversation?.users ?? [])];
  const bot = botProfile(snapshot);
  if (simulate && !users.some((user) => user.user_id === bot.user_id)) users.push({ ...bot, display_name: bot.display_name || "机器人" });
  return users;
}

/** The simulated speaker: a private chat's only user, else the chosen member (never the bot). */
export function resolveIdentity(conversation: Conversation | null, snapshot: Snapshot | null, identityId: string): string {
  const users = conversation?.users ?? [];
  if (!users.length) return "";
  if (conversation?.kind === "private") return users[0].user_id;
  const bot = botProfile(snapshot).user_id;
  return identityId && identityId !== bot && users.some((user) => user.user_id === identityId) ? identityId : users[0].user_id;
}

export const isKeyboard = (segment: Segment) => segment.type === "platform_specific" && segment.kind === "keyboard";

export function draftLabel(segment: Segment, users: SandboxUser[]): string {
  if (segment.type === "mention_user") return `@${users.find((user) => user.user_id === segment.user_id)?.display_name || segment.user_id}`;
  if (segment.type === "mention_all") return "@全体成员";
  if (segment.type === "markdown") return "[Markdown]";
  const name = typeof segment.payload.name === "string" ? segment.payload.name : "";
  if (segment.kind === "media") return `[${name || "媒体"}]`;
  if (segment.kind === "sticker") return `[${name || "表情包"}]`;
  if (segment.kind === "face") return "[表情]";
  if (segment.kind === "keyboard") return "[按钮]";
  return `[${segment.kind}]`;
}

export type ComposeResult = { ok: true; text: string; segments: Segment[] } | { ok: false; reason: "empty" | "markdownMixed" | "keyboardNeedsMarkdown" };

/** Markdown messages carry only the markdown and optional buttons; buttons need markdown. */
export function composePayload(draft: string, segments: Segment[], markdown: boolean): ComposeResult {
  const text = draft.trim();
  const keyboard = segments.some(isKeyboard);
  if (markdown) {
    if (segments.some((segment) => !isKeyboard(segment) && segment.type !== "markdown")) return { ok: false, reason: "markdownMixed" };
    if (!text) return { ok: false, reason: keyboard ? "keyboardNeedsMarkdown" : "empty" };
    return { ok: true, text: "", segments: [{ type: "markdown", content: text }, ...segments.filter(isKeyboard)] };
  }
  if (!text && !segments.length) return { ok: false, reason: "empty" };
  if (keyboard) return { ok: false, reason: "keyboardNeedsMarkdown" };
  return { ok: true, text, segments };
}

/** `文字 | 指令` per line → a QQ keyboard segment (five buttons per row), or null when empty. */
export function keyboardSegment(source: string): Segment | null {
  const buttons = source.split("\n").flatMap((line, index) => {
    const [label, data] = line.split("|").map((part) => part.trim());
    if (!label) return [];
    return [{ id: `btn_${index + 1}`, render_data: { label, style: 1 }, action: { type: 2, permission: { type: 2 }, data: data || label, enter: true } }];
  });
  if (!buttons.length) return null;
  const rows = [];
  for (let index = 0; index < buttons.length; index += 5) rows.push({ buttons: buttons.slice(index, index + 5) });
  return { type: "platform_specific", platform: "qqbot", kind: "keyboard", payload: { content: { rows } } };
}

export function keyboardSource(segment: Segment | undefined): string {
  if (!segment || segment.type !== "platform_specific") return "";
  const rows = ((segment.payload.content as { rows?: unknown } | undefined)?.rows ?? []) as Array<{
    buttons?: Array<{ render_data?: { label?: string }; action?: { data?: string } }>;
  }>;
  return rows
    .flatMap((row) => row.buttons ?? [])
    .map((item) => {
      const label = item.render_data?.label ?? "";
      const data = item.action?.data ?? "";
      return data && data !== label ? `${label} | ${data}` : label;
    })
    .join("\n");
}

/** The `@query` at the end of the draft, if the user is typing a mention. */
export function mentionQuery(draft: string): { index: number; query: string } | null {
  const match = /@([^\s@]*)$/.exec(draft);
  return match ? { index: match.index, query: match[1].toLowerCase() } : null;
}

export async function fileToBase64(file: Blob): Promise<string> {
  const bytes = new Uint8Array(await file.arrayBuffer());
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}
