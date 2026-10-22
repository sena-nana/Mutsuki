import type { MkTone } from "@mutsuki/ui";
import { formatDateTime, formatDuration, type JsonValue } from "@mutsuki/web-sdk";

import type { PlatformFieldFormat, PlatformHealth } from "./types";

export interface RenderedField {
  text: string;
  tone?: MkTone;
  mono?: boolean;
}

type Translate = (key: string) => string;

/** One descriptor field value rendered by its declared format. */
export function renderField(
  format: PlatformFieldFormat,
  value: JsonValue | undefined,
  t: Translate,
  now: number = Date.now(),
): RenderedField {
  if (value == null || value === "") return { text: t("unknown") };
  switch (format) {
    case "mono":
      return { text: String(value), mono: true };
    case "timestamp":
      return { text: formatDateTime(Number(value)) };
    case "since":
      return { text: Number(value) > 0 ? formatDuration(Math.max(0, now - Number(value))) : t("unknown") };
    case "count":
      return { text: String(Number(value)) };
    case "status":
      if (typeof value === "boolean") return { text: value ? t("yes") : t("no"), tone: value ? "ok" : "warn" };
      return { text: String(value) };
    case "secret_status": {
      const state = String(value);
      const tone: MkTone = state === "configured" ? "ok" : state === "restricted" ? "neutral" : "err";
      const known = ["configured", "missing", "invalid", "restricted"].includes(state);
      return { text: known ? t(`secret.${state}`) : state, tone };
    }
    default:
      return { text: typeof value === "object" ? JSON.stringify(value) : String(value) };
  }
}

export function healthTone(health: PlatformHealth): MkTone {
  return health === "ok" ? "ok" : health === "degraded" ? "warn" : "err";
}
