import type { JsonValue } from "@mutsuki/web-sdk";

export type CellKind = "null" | "blob" | "json" | "text";

/**
 * Display text for one SQLite cell. Rendered as text (never markup), so stored values cannot
 * inject elements or attributes.
 */
export function formatCell(value: JsonValue | undefined): { text: string; kind: CellKind } {
  if (value == null) return { text: "NULL", kind: "null" };
  if (typeof value === "object" && !Array.isArray(value) && value.$type === "blob") {
    return { text: `<blob ${Number(value.bytes) || 0} bytes>`, kind: "blob" };
  }
  if (typeof value === "object") return { text: JSON.stringify(value), kind: "json" };
  return { text: String(value), kind: "text" };
}
