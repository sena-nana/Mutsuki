/**
 * RuntimeEvent → timeline records. Owned by the diagnostics extension (moved from the agent
 * kit's trajectory model in W6); the agent console keeps its own AgentEvent projection.
 */
import type { RuntimeEvent, TaskEventPage } from "./contract";

export const EVENT_PAGE_LIMIT = 100;
export const EVENT_WINDOW_LIMIT = EVENT_PAGE_LIMIT * 3;
const DETAIL_CAP = 400;

export type RecordKind = "system" | "other" | "step" | "task" | "error";

const RUNTIME_KIND: Record<string, RecordKind> = {
  lifecycle: "system",
  plugin: "system",
  resource: "other",
  trace: "step",
  host: "system",
  task: "task",
  runner: "other",
  state: "step",
  effect: "other",
  reload: "system",
};

export interface TimelineRecord {
  id: string;
  index: number;
  kind: RecordKind;
  label: string;
  preview: string;
  groupId: string;
  groupLabel: string;
  status: "done" | "error";
  isError: boolean;
  input: string | null;
  output: string | null;
  sequences: number[];
}

export interface TimelineGroup {
  id: string;
  label: string;
  recordIds: string[];
}

export function sequenceOf(event: RuntimeEvent | null | undefined): number | null {
  const value = Number(event?.sequence);
  return event?.sequence != null && Number.isFinite(value) ? value : null;
}

export function eventsOf(page: TaskEventPage | null | undefined): RuntimeEvent[] {
  return page?.events ?? page?.items ?? [];
}

/** Union by sequence, newer copies win, ascending. Events without a sequence are dropped. */
export function mergeBySequence(existing: RuntimeEvent[], incoming: RuntimeEvent[]): RuntimeEvent[] {
  const bySequence = new Map<number, RuntimeEvent>();
  for (const event of [...existing, ...incoming]) {
    const sequence = sequenceOf(event);
    if (sequence != null) bySequence.set(sequence, event);
  }
  return [...bySequence.entries()].sort(([a], [b]) => a - b).map(([, event]) => event);
}

/** Bounds the loaded window, keeping the `keep` end. */
export function trimWindow<T>(events: T[], max = EVENT_WINDOW_LIMIT, keep: "older" | "newer" = "newer"): T[] {
  if (events.length <= max) return events;
  return keep === "older" ? events.slice(0, max) : events.slice(events.length - max);
}

export function compactText(value: unknown, max = 120): string {
  const text = String(value ?? "").replace(/\s+/g, " ").trim();
  if (text.length <= max) return text;
  return `${text.slice(0, Math.max(0, max - 1))}…`;
}

function stringify(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function scalar(value: unknown): string {
  if (value == null) return "—";
  return compactText(typeof value === "object" ? stringify(value) : String(value), 40);
}

function preview(name: string, attributes: Record<string, unknown>, error?: { message?: string } | null): string {
  if (error?.message) return `${name}: ${error.message}`;
  const keys = Object.keys(attributes);
  if (!keys.length) return name;
  return `${name} · ${keys.slice(0, 3).map((key) => `${key}=${scalar(attributes[key])}`).join(" · ")}`;
}

/** Projects runtime events, optionally keeping only one task's (subject's) events. */
export function projectRuntimeEvents(
  events: RuntimeEvent[],
  options: { focusSubjectId?: string | null } = {},
): { records: TimelineRecord[]; groups: TimelineGroup[] } {
  const focus = options.focusSubjectId ? String(options.focusSubjectId) : "";
  const records: TimelineRecord[] = [];
  for (const event of events) {
    const subject = event?.subject_id != null ? String(event.subject_id) : "";
    if (focus && subject && subject !== focus) continue;
    const sequence = sequenceOf(event);
    const runtimeKind = String(event?.kind || "task");
    const attributes = event?.attributes && typeof event.attributes === "object" ? event.attributes : {};
    const name = String(event?.name || runtimeKind);
    const isError = !!event?.error;
    records.push({
      id: `runtime:${sequence ?? records.length}`,
      index: records.length + 1,
      kind: isError ? "error" : (RUNTIME_KIND[runtimeKind] ?? "task"),
      label: name,
      preview: compactText(preview(name, attributes, event?.error)),
      groupId: subject || "runtime",
      groupLabel: subject ? `任务 ${subject}` : "运行时",
      status: isError ? "error" : "done",
      isError,
      input: Object.keys(attributes).length ? compactText(stringify(attributes), DETAIL_CAP) : null,
      output: isError ? compactText(stringify(event.error), DETAIL_CAP) : null,
      sequences: sequence == null ? [] : [sequence],
    });
  }
  return { records, groups: groupRecords(records) };
}

export function groupRecords(records: TimelineRecord[]): TimelineGroup[] {
  const groups = new Map<string, TimelineGroup>();
  for (const record of records) {
    let group = groups.get(record.groupId);
    if (!group) {
      group = { id: record.groupId, label: record.groupLabel, recordIds: [] };
      groups.set(record.groupId, group);
    }
    group.recordIds.push(record.id);
  }
  return [...groups.values()];
}
