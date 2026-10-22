import type { BotTarget } from "./contract";

type Translate = (key: string, params?: Record<string, string | number>) => string;

export function formatTarget(target: BotTarget | null | undefined, t: Translate): string {
  switch (target?.type) {
    case "group":
      return t("target.group", { id: target.group_id });
    case "user":
      return t("target.user", { id: target.user_id });
    case "guild_channel":
      return t("target.channel", { guild: target.guild_id, channel: target.channel_id });
    case "conversation":
      return t("target.conversation", { id: target.conversation_id });
    case "platform_specific":
      return `${target.platform}:${target.kind}:${target.id}`;
    default:
      return t("target.unknown");
  }
}

/** Only http(s) links from the preview card become clickable. */
export function safeHttpUrl(value: string | null | undefined): string {
  if (!value) return "";
  try {
    const parsed = new URL(value);
    return parsed.protocol === "http:" || parsed.protocol === "https:" ? parsed.href : "";
  } catch {
    return "";
  }
}
