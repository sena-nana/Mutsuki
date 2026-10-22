/**
 * AgentEvent envelopes → ledger records (assistant/thinking streams merged per turn, tool and
 * command calls merged by id, usage folded into the turn's reply). Ported from the agent kit's
 * `trajectory-model.js`, which the legacy pages still use until W11 removes them.
 */
import type { MkLedgerGroup, MkLedgerRecord } from "@mutsuki/ui";
import type { JsonValue } from "@mutsuki/web-sdk";

import type { AgentEventEnvelope } from "./contract";

const DETAIL_CAP = 400;

export const KIND_LABELS: Record<string, string> = {
  user: "用户",
  assistant: "助手",
  thinking: "思考",
  tool: "工具",
  subtool: "子工具",
  command: "命令",
  file: "文件",
  approval: "批准",
  interaction: "提问",
  error: "错误",
  usage: "用量",
  step: "步骤",
  system: "系统",
  plan: "计划",
  todo: "待办",
  context: "上下文",
  other: "其他",
};

const TYPE_KIND: Record<string, string> = {
  user_message: "user",
  approval_request: "approval",
  interaction_requested: "interaction",
  interaction_resolved: "interaction",
  warning: "system",
  error: "error",
  failed: "error",
  cancelled: "error",
  session_state: "system",
  turn_state: "step",
  step_state: "step",
  context_status: "context",
  context_usage_updated: "context",
  plan_updated: "plan",
  todo_updated: "todo",
  file_change_proposed: "file",
  file_change_applied: "file",
  file_change_rejected: "file",
  workspace_edit_proposed: "file",
  sub_agent_status: "subtool",
};

export interface AgentRecord extends MkLedgerRecord {
  groupLabel: string;
  callId: string | null;
  startedAt: number | null;
  endedAt: number | null;
  ttftMs: number | null;
  tokens: { input: number; output: number; total: number } | null;
  sequences: number[];
}

type Event = Record<string, JsonValue>;

export function compactText(value: unknown, max = 120): string {
  const text = String(value ?? "").replace(/\s+/g, " ").trim();
  return text.length <= max ? text : `${text.slice(0, Math.max(0, max - 1))}…`;
}

function appendCapped(current: string | null | undefined, next: string, cap = DETAIL_CAP): string {
  return `${current ?? ""}${next}`.slice(0, cap);
}

function stringify(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function detail(value: unknown): string | null {
  return value == null || value === "" ? null : compactText(value, DETAIL_CAP);
}

function timestampOf(meta: Record<string, JsonValue>): number | null {
  const value = Number(meta.timestamp_unix_ms ?? 0);
  return value > 0 ? value : null;
}

function shortId(value: string): string {
  return value.length > 12 ? `${value.slice(0, 8)}…` : value;
}

export function groupLabelFor(groupId: string): string {
  return !groupId || groupId === "session" ? "会话" : `第 ${shortId(groupId)} 轮`;
}

function previewFor(type: string, event: Event, meta: Record<string, JsonValue>): string {
  for (const value of [meta.summary, event.content, event.text, event.summary, event.status, event.message]) {
    if (value) return compactText(value);
  }
  const error = event.error as Event | null | undefined;
  if (error?.message) return compactText(error.message);
  if (event.prompt) return compactText(event.prompt);
  for (const key of ["plan", "todo", "change", "proposal"]) {
    if (event[key]) return compactText(stringify(event[key]));
  }
  return compactText(type);
}

function inputFor(type: string, event: Event): string | null {
  if (type === "user_message") return event.content != null ? String(event.content) : null;
  for (const key of ["request", "interaction", "change", "proposal"]) {
    if (event[key]) return stringify(event[key]);
  }
  return null;
}

function outputFor(type: string, event: Event): string | null {
  for (const key of ["error", "resolution", "result"]) {
    if (event[key]) return stringify(event[key]);
  }
  if (event.summary && type !== "user_message") return String(event.summary);
  if (event.status && ["turn_state", "step_state", "session_state"].includes(type)) return String(event.status);
  return null;
}

const SALIENT_SESSION_STATES = new Set(["failed", "closed", "cancelled", "error", "degraded"]);

function record(init: Partial<AgentRecord> & Pick<AgentRecord, "id" | "kind" | "groupId">): AgentRecord {
  return {
    index: 0,
    label: KIND_LABELS[init.kind] ?? "记录",
    preview: "",
    groupLabel: groupLabelFor(init.groupId),
    isError: false,
    status: "done",
    parentId: null,
    callId: null,
    startedAt: null,
    endedAt: init.startedAt ?? null,
    durationMs: null,
    ttftMs: null,
    tokens: null,
    input: null,
    output: null,
    thinking: null,
    sequences: [],
    ...init,
  };
}

function touch(target: AgentRecord, sequence: number | null, time: number | null): void {
  if (sequence != null && !target.sequences.includes(sequence)) target.sequences.push(sequence);
  if (time != null) {
    if (target.startedAt == null || time < target.startedAt) target.startedAt = time;
    target.endedAt = target.endedAt == null ? time : Math.max(target.endedAt, time);
  }
}

export function projectAgentEvents(envelopes: AgentEventEnvelope[]): {
  records: AgentRecord[];
  groups: MkLedgerGroup[];
} {
  const records: AgentRecord[] = [];
  const byId = new Map<string, AgentRecord>();
  const assistantByTurn = new Map<string, AgentRecord>();
  const thinkingByTurn = new Map<string, AgentRecord>();
  const toolByCall = new Map<string, AgentRecord>();
  const commandById = new Map<string, AgentRecord>();
  const turnStart = new Map<string, number>();
  const firstToken = new Map<string, number>();
  const push = (next: AgentRecord) => {
    records.push(next);
    byId.set(next.id, next);
    return next;
  };
  const parentOf = (meta: Record<string, JsonValue>): string | null => {
    const parentEvent = meta.parent_event_id ? String(meta.parent_event_id) : "";
    if (parentEvent && byId.has(parentEvent)) return parentEvent;
    const parentTool = meta.tool_id ? String(meta.tool_id) : "";
    return parentTool && toolByCall.has(parentTool) ? `tool:${parentTool}` : null;
  };

  for (const envelope of envelopes) {
    const event = (envelope.event ?? {}) as Event;
    const meta = (envelope.meta ?? {}) as Record<string, JsonValue>;
    const type = String(event.type ?? "");
    const sequence = Number.isFinite(Number(envelope.sequence)) ? Number(envelope.sequence) : null;
    const time = timestampOf(meta);
    const turn = String(event.turn_id ?? meta.turn_id ?? "session");
    if (time != null && (!turnStart.has(turn) || time < turnStart.get(turn)!)) turnStart.set(turn, time);

    if (type === "model_delta" || type === "reasoning_delta") {
      const thinking = type === "reasoning_delta";
      const map = thinking ? thinkingByTurn : assistantByTurn;
      const target =
        map.get(turn) ??
        push(record({ id: `${thinking ? "thinking" : "assistant"}:${turn}`, kind: thinking ? "thinking" : "assistant", groupId: turn, startedAt: time, status: "running" }));
      map.set(turn, target);
      const text = String(event.text ?? "");
      if (thinking) {
        target.thinking = appendCapped(target.thinking, text);
        target.preview = compactText(target.thinking);
      } else {
        target.output = appendCapped(target.output, text);
        target.preview = compactText(target.output);
      }
      touch(target, sequence, time);
      if (time != null && !firstToken.has(turn)) firstToken.set(turn, time);
      continue;
    }

    if (type === "final" || type === "final_response") {
      const target = assistantByTurn.get(turn) ?? push(record({ id: `assistant:${turn}`, kind: "assistant", groupId: turn, startedAt: time }));
      assistantByTurn.set(turn, target);
      if (event.summary) {
        target.output = target.output || compactText(event.summary, DETAIL_CAP);
        target.preview = compactText(event.summary);
      }
      target.status = "done";
      touch(target, sequence, time);
      continue;
    }

    if (["tool_call", "tool_call_started", "tool_result", "tool_call_completed"].includes(type)) {
      const callId = String(event.call_id ?? meta.tool_id ?? `tool:${sequence}`);
      let target = toolByCall.get(callId);
      if (!target) {
        const parentId = parentOf(meta);
        target = push(
          record({
            id: `tool:${callId}`,
            kind: parentId ? "subtool" : "tool",
            groupId: turn,
            parentId,
            callId,
            startedAt: time,
            status: "running",
            label: String(event.name ?? meta.summary ?? "工具"),
            preview: compactText(event.name ?? meta.summary ?? ""),
          }),
        );
        toolByCall.set(callId, target);
      }
      if (event.name) target.label = String(event.name);
      if (event.input != null) target.input = compactText(stringify(event.input), DETAIL_CAP);
      if (type === "tool_result" || type === "tool_call_completed") {
        target.output = compactText(event.summary ?? "", DETAIL_CAP);
        target.preview = compactText(target.output || target.label);
        target.status = "done";
      }
      touch(target, sequence, time);
      continue;
    }

    if (["command_started", "command_output", "command_exited"].includes(type)) {
      const command = (event.command ?? {}) as Event;
      const commandId = String(event.command_id ?? command.id ?? `cmd:${sequence}`);
      let target = commandById.get(commandId);
      if (!target) {
        target = push(
          record({
            id: `command:${commandId}`,
            kind: "command",
            groupId: turn,
            startedAt: time,
            status: type === "command_exited" ? "done" : "running",
            label: String(command.name ?? command.command ?? "命令"),
            preview: compactText(command.command ?? command.name ?? ""),
            input: command.command ? String(command.command) : null,
          }),
        );
        commandById.set(commandId, target);
      }
      if (type === "command_output") {
        target.output = appendCapped(target.output, String(event.chunk ?? ""));
        target.preview = compactText(target.output || target.preview);
      }
      if (type === "command_exited") {
        const failed = Number(event.exit_code) !== 0;
        target.status = failed ? "error" : "done";
        target.isError = failed;
        target.output = compactText([target.output, event.summary].filter(Boolean).join("\n"), DETAIL_CAP);
        target.preview = compactText(event.summary || target.output || target.preview);
      }
      touch(target, sequence, time);
      continue;
    }

    if (type === "usage") {
      const usage = (event.usage ?? {}) as Event;
      const tokens = {
        input: Number(usage.input_tokens ?? 0),
        output: Number(usage.output_tokens ?? 0),
        total: Number(usage.total_tokens ?? 0),
      };
      const assistant = assistantByTurn.get(turn);
      if (assistant) {
        assistant.tokens = tokens;
        touch(assistant, sequence, time);
      } else {
        push(
          record({
            id: `usage:${turn}:${sequence}`,
            kind: "usage",
            groupId: turn,
            startedAt: time,
            tokens,
            preview: `${tokens.total} tokens`,
            sequences: sequence == null ? [] : [sequence],
          }),
        );
      }
      continue;
    }

    if (type === "session_state" && !SALIENT_SESSION_STATES.has(String(event.status ?? "").toLowerCase())) continue;

    const kind = TYPE_KIND[type] ?? "other";
    const parentId = parentOf(meta);
    push(
      record({
        id: meta.event_id ? String(meta.event_id) : `${type}:${turn}:${sequence ?? "x"}`,
        kind: parentId && kind === "subtool" ? "subtool" : kind,
        groupId: turn,
        startedAt: time,
        status: kind === "error" ? "error" : "done",
        isError: kind === "error",
        label: KIND_LABELS[kind] ?? type,
        preview: previewFor(type, event, meta),
        input: detail(inputFor(type, event)),
        output: detail(outputFor(type, event)),
        sequences: sequence == null ? [] : [sequence],
        parentId,
      }),
    );
  }

  for (const item of records) {
    if (item.startedAt != null && item.endedAt != null && item.endedAt >= item.startedAt) {
      item.durationMs = item.endedAt - item.startedAt;
    }
    if (item.kind === "assistant") {
      const start = turnStart.get(item.groupId);
      const token = firstToken.get(item.groupId);
      if (start != null && token != null && token >= start) item.ttftMs = token - start;
      if (item.startedAt == null && start != null) item.startedAt = start;
    }
  }
  records.forEach((item, index) => (item.index = index + 1));

  const groups = new Map<string, MkLedgerGroup>();
  for (const item of records) {
    if (!groups.has(item.groupId)) groups.set(item.groupId, { id: item.groupId, label: item.groupLabel });
  }
  return { records, groups: [...groups.values()] };
}

/** Union by sequence, newer copies win, ascending. */
export function mergeBySequence(existing: AgentEventEnvelope[], incoming: AgentEventEnvelope[]): AgentEventEnvelope[] {
  const bySequence = new Map<number, AgentEventEnvelope>();
  for (const event of [...existing, ...incoming]) bySequence.set(event.sequence, event);
  return [...bySequence.entries()].sort(([a], [b]) => a - b).map(([, event]) => event);
}
