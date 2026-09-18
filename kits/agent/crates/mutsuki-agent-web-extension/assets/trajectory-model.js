/** Shared trajectory records projected from AgentEvent / RuntimeEvent streams. */

export const EVENT_PAGE_LIMIT = 100;
export const EVENT_WINDOW_LIMIT = EVENT_PAGE_LIMIT * 3;
const DETAIL_CAP = 400;

export const KIND_LABELS = {
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
  task: "任务",
  other: "其他",
};

const AGENT_TYPE_KIND = {
  user_message: "user",
  model_delta: "assistant",
  reasoning_delta: "thinking",
  tool_call: "tool",
  tool_call_started: "tool",
  tool_result: "tool",
  tool_call_completed: "tool",
  approval_request: "approval",
  interaction_requested: "interaction",
  interaction_resolved: "interaction",
  usage: "usage",
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
  command_started: "command",
  command_output: "command",
  command_exited: "command",
  file_change_proposed: "file",
  file_change_applied: "file",
  file_change_rejected: "file",
  workspace_edit_proposed: "file",
  diagnostic_updated: "other",
  test_or_build_result: "other",
  artifact_produced: "other",
  sub_agent_status: "subtool",
  final: "assistant",
  final_response: "assistant",
};

const RUNTIME_KIND_MAP = {
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

export function formatDurationMs(ms) {
  if (ms == null || Number.isNaN(Number(ms))) return "—";
  const value = Math.max(0, Math.round(Number(ms)));
  if (value < 1000) return `${value} ms`;
  if (value < 60_000) return `${(value / 1000).toFixed(value < 10_000 ? 2 : 1)} s`;
  const minutes = Math.floor(value / 60_000);
  const seconds = Math.round((value % 60_000) / 1000);
  return `${minutes} m ${seconds} s`;
}

export function formatClock(ms) {
  if (ms == null || Number.isNaN(Number(ms)) || Number(ms) <= 0) return "—";
  const date = new Date(Number(ms));
  if (Number.isNaN(date.getTime())) return "—";
  const pad = (n) => String(n).padStart(2, "0");
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}.${String(
    date.getMilliseconds(),
  ).padStart(3, "0")}`;
}

export function appendCapped(current, next, cap = DETAIL_CAP) {
  const text = `${current || ""}${next || ""}`;
  if (text.length <= cap) return text;
  return text.slice(0, cap);
}

function compactDetail(value) {
  if (value == null || value === "") return null;
  return compactText(value, DETAIL_CAP);
}

export function compactText(value, max = 120) {
  const text = String(value ?? "")
    .replace(/\s+/g, " ")
    .trim();
  if (!text) return "";
  if (text.length <= max) return text;
  return `${text.slice(0, Math.max(0, max - 1))}…`;
}

export function asEventList(body) {
  return asEventPage(body).events;
}

export function asEventPage(body) {
  const events = Array.isArray(body)
    ? body
    : Array.isArray(body?.events)
      ? body.events
      : Array.isArray(body?.items)
        ? body.items
        : [];
  const sequences = events.map(sequenceOf).filter((value) => value != null);
  return {
    events,
    next_sequence: Number(body?.next_sequence || (sequences.length ? Math.max(...sequences) : 0)),
    lost: Number(body?.lost || 0),
    truncated: !!(body?.truncated || body?.has_more),
  };
}

export function trimWindow(events, maxCount = EVENT_WINDOW_LIMIT, keep = "newer") {
  const list = events || [];
  if (list.length <= maxCount) return list;
  return keep === "older" ? list.slice(0, maxCount) : list.slice(list.length - maxCount);
}

export function mergeBySequence(existing, incoming) {
  const map = new Map();
  for (const event of existing || []) {
    const key = sequenceOf(event);
    if (key != null) map.set(key, event);
  }
  for (const event of incoming || []) {
    const key = sequenceOf(event);
    if (key == null) continue;
    map.set(key, event);
  }
  return [...map.values()].sort((a, b) => sequenceOf(a) - sequenceOf(b));
}

export function sequenceOf(event) {
  const value = Number(event?.sequence);
  return Number.isFinite(value) ? value : null;
}

export function groupRecords(records) {
  const groups = [];
  const index = new Map();
  for (const record of records || []) {
    const id = record.groupId || "session";
    let group = index.get(id);
    if (!group) {
      group = {
        id,
        label: record.groupLabel || id,
        recordIds: [],
        startedAt: record.startedAt ?? null,
        endedAt: record.endedAt ?? null,
      };
      index.set(id, group);
      groups.push(group);
    }
    group.recordIds.push(record.id);
    if (record.startedAt != null) {
      group.startedAt =
        group.startedAt == null ? record.startedAt : Math.min(group.startedAt, record.startedAt);
    }
    const end = record.endedAt ?? record.startedAt;
    if (end != null) {
      group.endedAt = group.endedAt == null ? end : Math.max(group.endedAt, end);
    }
  }
  return groups;
}

export function projectAgentEvents(events) {
  const list = asEventList(events);
  const records = [];
  const byId = new Map();
  const assistantByTurn = new Map();
  const thinkingByTurn = new Map();
  const toolByCall = new Map();
  const commandById = new Map();
  const turnStart = new Map();
  const firstToken = new Map();

  const push = (record) => {
    records.push(record);
    byId.set(record.id, record);
    return record;
  };

  for (const entry of list) {
    const event = entry?.event && typeof entry.event === "object" ? entry.event : entry || {};
    const type = String(event.type || "");
    const meta = entry?.meta && typeof entry.meta === "object" ? entry.meta : {};
    const seq = sequenceOf(entry);
    const time = timestampOf(meta);
    const turn = String(event.turn_id || meta.turn_id || "session");
    rememberTurnStart(turnStart, turn, time);
    const kind = AGENT_TYPE_KIND[type] || "other";

    if (type === "model_delta") {
      const record =
        assistantByTurn.get(turn) ||
        push(
          createRecord({
            id: `assistant:${turn}`,
            kind: "assistant",
            groupId: turn,
            startedAt: time,
            status: "running",
          }),
        );
      assistantByTurn.set(turn, record);
      const text = String(event.text || "");
      record.output = appendCapped(record.output, text);
      record.preview = compactText(record.output);
      touch(record, seq, time);
      if (time != null && !firstToken.has(turn)) firstToken.set(turn, time);
      continue;
    }

    if (type === "reasoning_delta") {
      const record =
        thinkingByTurn.get(turn) ||
        push(
          createRecord({
            id: `thinking:${turn}`,
            kind: "thinking",
            groupId: turn,
            startedAt: time,
            status: "running",
          }),
        );
      thinkingByTurn.set(turn, record);
      record.thinking = appendCapped(record.thinking, String(event.text || ""));
      record.preview = compactText(record.thinking);
      touch(record, seq, time);
      if (time != null && !firstToken.has(turn)) firstToken.set(turn, time);
      continue;
    }

    if (type === "final" || type === "final_response") {
      const record =
        assistantByTurn.get(turn) ||
        push(
          createRecord({
            id: `assistant:${turn}`,
            kind: "assistant",
            groupId: turn,
            startedAt: time,
          }),
        );
      assistantByTurn.set(turn, record);
      if (event.summary) {
        record.output = record.output || compactText(event.summary, DETAIL_CAP);
        record.preview = compactText(event.summary);
      }
      record.status = "done";
      touch(record, seq, time);
      continue;
    }

    if (
      type === "tool_call" ||
      type === "tool_call_started" ||
      type === "tool_result" ||
      type === "tool_call_completed"
    ) {
      const callId = String(event.call_id || meta.tool_id || `tool:${seq}`);
      const parentId = parentToolId(meta, byId, toolByCall);
      let record = toolByCall.get(callId);
      if (!record) {
        record = push(
          createRecord({
            id: `tool:${callId}`,
            kind: parentId ? "subtool" : "tool",
            groupId: turn,
            parentId,
            callId,
            startedAt: time,
            status: "running",
            label: String(event.name || meta.summary || "工具"),
            preview: compactText(event.name || meta.summary || ""),
            input: event.input != null ? compactText(stringify(event.input), DETAIL_CAP) : null,
          }),
        );
        toolByCall.set(callId, record);
      }
      if (event.name) record.label = String(event.name);
      if (event.input != null) record.input = compactText(stringify(event.input), DETAIL_CAP);
      if (type === "tool_result" || type === "tool_call_completed") {
        record.output = compactText(event.summary || "", DETAIL_CAP);
        record.preview = compactText(record.output || record.label);
        record.status = "done";
      }
      touch(record, seq, time);
      continue;
    }

    if (type === "command_started" || type === "command_output" || type === "command_exited") {
      const command = event.command || {};
      const commandId = String(event.command_id || command.id || `cmd:${seq}`);
      let record = commandById.get(commandId);
      if (!record) {
        record = push(
          createRecord({
            id: `command:${commandId}`,
            kind: "command",
            groupId: turn,
            startedAt: time,
            status: type === "command_exited" ? "done" : "running",
            label: String(command.name || command.command || "命令"),
            preview: compactText(command.command || command.name || ""),
            input: command.command ? String(command.command) : null,
          }),
        );
        commandById.set(commandId, record);
      }
      if (type === "command_output") {
        record.output = appendCapped(record.output, event.chunk || "");
        record.preview = compactText(record.output || record.preview);
      }
      if (type === "command_exited") {
        record.status = Number(event.exit_code) === 0 ? "done" : "error";
        record.isError = Number(event.exit_code) !== 0;
        record.output = compactText([record.output, event.summary].filter(Boolean).join("\n"), DETAIL_CAP);
        record.preview = compactText(event.summary || record.output || record.preview);
      }
      touch(record, seq, time);
      continue;
    }

    if (type === "usage") {
      const usage = event.usage || {};
      const tokens = {
        input: Number(usage.input_tokens || 0),
        output: Number(usage.output_tokens || 0),
        total: Number(usage.total_tokens || 0),
      };
      const assistant = assistantByTurn.get(turn);
      if (assistant) {
        assistant.tokens = tokens;
        touch(assistant, seq, time);
      } else {
        push(
          createRecord({
            id: `usage:${turn}:${seq}`,
            kind: "usage",
            groupId: turn,
            startedAt: time,
            endedAt: time,
            tokens,
            preview: `${tokens.total} tokens`,
            sequences: seq == null ? [] : [seq],
          }),
        );
      }
      continue;
    }

    if (type === "session_state" && !isSalientStatus(event.status)) continue;

    const record = push(
      createRecord({
        id: recordIdFor(type, turn, seq, meta),
        kind,
        groupId: turn,
        startedAt: time,
        endedAt: time,
        status: kind === "error" ? "error" : "done",
        isError: kind === "error",
        label: KIND_LABELS[kind] || type,
        preview: previewForAgent(type, event, meta),
        input: compactDetail(inputForAgent(type, event)),
        output: compactDetail(outputForAgent(type, event)),
        sequences: seq == null ? [] : [seq],
        parentId: parentToolId(meta, byId, toolByCall),
      }),
    );
    if (record.kind === "subtool" || record.parentId) record.kind = record.parentId ? "subtool" : record.kind;
  }

  for (const record of records) {
    if (record.startedAt != null && record.endedAt != null && record.endedAt >= record.startedAt) {
      record.durationMs = record.endedAt - record.startedAt;
    }
    if (record.kind === "assistant") {
      const start = turnStart.get(record.groupId);
      const token = firstToken.get(record.groupId);
      if (start != null && token != null && token >= start) record.ttftMs = token - start;
      if (record.startedAt == null && start != null) record.startedAt = start;
    }
    record.groupLabel = groupLabelFor(record.groupId);
  }

  assignIndexes(records);
  return {
    records,
    groups: groupRecords(records),
    sourceCount: list.length,
  };
}

export function projectRuntimeEvents(events, options = {}) {
  const list = asEventList(events);
  const focus = options.focusSubjectId ? String(options.focusSubjectId) : "";
  const records = [];
  for (const event of list) {
    const subject = event?.subject_id != null ? String(event.subject_id) : "";
    if (focus && subject && subject !== focus) continue;
    const seq = sequenceOf(event);
    const runtimeKind = String(event?.kind || "task");
    const kind = event?.error ? "error" : RUNTIME_KIND_MAP[runtimeKind] || "task";
    const name = String(event?.name || runtimeKind);
    const attributes = event?.attributes && typeof event.attributes === "object" ? event.attributes : {};
    records.push(
      createRecord({
        id: `runtime:${seq ?? records.length}`,
        kind,
        groupId: subject || "runtime",
        groupLabel: subject ? `任务 ${subject}` : "运行时",
        startedAt: null,
        endedAt: null,
        durationMs: null,
        status: event?.error ? "error" : "done",
        isError: !!event?.error,
        label: name,
        preview: compactText(previewForRuntime(name, attributes, event?.error)),
        input: Object.keys(attributes).length ? compactText(stringify(attributes), DETAIL_CAP) : null,
        output: event?.error ? compactText(stringify(event.error), DETAIL_CAP) : null,
        sequences: seq == null ? [] : [seq],
      }),
    );
  }
  assignIndexes(records);
  return {
    records,
    groups: groupRecords(records),
    sourceCount: list.length,
  };
}

function createRecord(init) {
  return {
    id: init.id,
    index: 0,
    kind: init.kind || "other",
    label: init.label || KIND_LABELS[init.kind] || "记录",
    preview: init.preview || "",
    groupId: init.groupId || "session",
    groupLabel: init.groupLabel || groupLabelFor(init.groupId || "session"),
    parentId: init.parentId || null,
    callId: init.callId || null,
    startedAt: init.startedAt ?? null,
    endedAt: init.endedAt ?? init.startedAt ?? null,
    durationMs: init.durationMs ?? null,
    ttftMs: init.ttftMs ?? null,
    tokens: init.tokens || null,
    input: init.input || null,
    output: init.output || null,
    thinking: init.thinking || null,
    raw: init.raw || [],
    sequences: init.sequences || [],
    isError: !!init.isError,
    status: init.status || "done",
  };
}

function touch(record, seq, time) {
  if (seq != null && !record.sequences.includes(seq)) record.sequences.push(seq);
  if (time != null) {
    if (record.startedAt == null || time < record.startedAt) record.startedAt = time;
    record.endedAt = record.endedAt == null ? time : Math.max(record.endedAt, time);
  }
}

function timestampOf(meta) {
  const value = Number(meta?.timestamp_unix_ms || 0);
  return value > 0 ? value : null;
}

function rememberTurnStart(map, turn, time) {
  if (time == null) return;
  const current = map.get(turn);
  if (current == null || time < current) map.set(turn, time);
}

function parentToolId(meta, byId, toolByCall) {
  const parentEvent = meta?.parent_event_id ? String(meta.parent_event_id) : "";
  if (parentEvent && byId.has(parentEvent)) return parentEvent;
  const parentTool = meta?.tool_id ? String(meta.tool_id) : "";
  if (parentTool && toolByCall.has(parentTool)) return `tool:${parentTool}`;
  return null;
}

function recordIdFor(type, turn, seq, meta) {
  if (meta?.event_id) return String(meta.event_id);
  return `${type}:${turn}:${seq ?? "x"}`;
}

function previewForAgent(type, event, meta) {
  if (meta?.summary) return compactText(meta.summary);
  if (event.content) return compactText(event.content);
  if (event.text) return compactText(event.text);
  if (event.summary) return compactText(event.summary);
  if (event.status) return compactText(event.status);
  if (event.message) return compactText(event.message);
  if (event.error?.message) return compactText(event.error.message);
  if (event.prompt) return compactText(event.prompt);
  if (event.plan) return compactText(stringify(event.plan));
  if (event.todo) return compactText(stringify(event.todo));
  if (event.change) return compactText(stringify(event.change));
  if (event.proposal) return compactText(stringify(event.proposal));
  if (event.status && event.child_run_id) return compactText(`${event.child_run_id} ${event.status}`);
  return compactText(type);
}

function inputForAgent(type, event) {
  if (type === "user_message") return event.content != null ? String(event.content) : null;
  if (event.request) return stringify(event.request);
  if (event.interaction) return stringify(event.interaction);
  if (event.change) return stringify(event.change);
  if (event.proposal) return stringify(event.proposal);
  return null;
}

function outputForAgent(type, event) {
  if (event.error) return stringify(event.error);
  if (event.resolution) return stringify(event.resolution);
  if (event.result) return stringify(event.result);
  if (event.summary && type !== "user_message") return String(event.summary);
  if (event.status && (type === "turn_state" || type === "step_state" || type === "session_state")) {
    return String(event.status);
  }
  return null;
}

function previewForRuntime(name, attributes, error) {
  if (error?.message) return `${name}: ${error.message}`;
  const keys = Object.keys(attributes);
  if (!keys.length) return name;
  const bits = keys.slice(0, 3).map((key) => `${key}=${formatScalar(attributes[key])}`);
  return `${name} · ${bits.join(" · ")}`;
}

function formatScalar(value) {
  if (value == null) return "—";
  if (typeof value === "object") return compactText(stringify(value), 40);
  return compactText(String(value), 40);
}

function stringify(value) {
  try {
    return JSON.stringify(value, null, 2);
  } catch (_error) {
    return String(value);
  }
}

function isSalientStatus(status) {
  const value = String(status || "").toLowerCase();
  return ["failed", "closed", "cancelled", "error", "degraded"].includes(value);
}

function groupLabelFor(groupId) {
  if (!groupId || groupId === "session" || groupId === "runtime") return "会话";
  if (String(groupId).startsWith("任务 ")) return groupId;
  return `第 ${shortId(groupId)} 轮`;
}

function shortId(value) {
  const text = String(value);
  return text.length > 12 ? `${text.slice(0, 8)}…` : text;
}

function assignIndexes(records) {
  records.forEach((record, index) => {
    record.index = index + 1;
    if (!record.label) record.label = KIND_LABELS[record.kind] || "记录";
  });
}
