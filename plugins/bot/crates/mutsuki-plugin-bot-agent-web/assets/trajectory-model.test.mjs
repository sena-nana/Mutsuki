import assert from "node:assert/strict";
import {
  asEventPage,
  formatDurationMs,
  projectAgentEvents,
  projectRuntimeEvents,
  trimWindow,
} from "./trajectory-model.js";

const projected = projectAgentEvents([
  {
    sequence: 1,
    meta: { timestamp_unix_ms: 1_000, turn_id: "t1" },
    event: { type: "user_message", turn_id: "t1", content: "hello" },
  },
  {
    sequence: 2,
    meta: { timestamp_unix_ms: 1_100, turn_id: "t1" },
    event: { type: "reasoning_delta", turn_id: "t1", text: "think " },
  },
  {
    sequence: 3,
    meta: { timestamp_unix_ms: 1_150, turn_id: "t1" },
    event: { type: "reasoning_delta", turn_id: "t1", text: "more" },
  },
  {
    sequence: 4,
    meta: { timestamp_unix_ms: 1_200, turn_id: "t1" },
    event: { type: "model_delta", turn_id: "t1", text: "Hel" },
  },
  {
    sequence: 5,
    meta: { timestamp_unix_ms: 1_300, turn_id: "t1" },
    event: { type: "model_delta", turn_id: "t1", text: "lo" },
  },
  {
    sequence: 6,
    meta: { timestamp_unix_ms: 1_400, turn_id: "t1" },
    event: { type: "tool_call_started", turn_id: "t1", call_id: "c1", name: "read", input: { path: "a" } },
  },
  {
    sequence: 7,
    meta: { timestamp_unix_ms: 1_500, turn_id: "t1", parent_event_id: "tool:c1" },
    event: { type: "tool_call_started", turn_id: "t1", call_id: "c2", name: "inner", input: {} },
  },
  {
    sequence: 8,
    meta: { timestamp_unix_ms: 1_800, turn_id: "t1" },
    event: { type: "tool_call_completed", turn_id: "t1", call_id: "c1", summary: "ok" },
  },
  {
    sequence: 9,
    meta: { timestamp_unix_ms: 1_900, turn_id: "t1" },
    event: { type: "usage", turn_id: "t1", usage: { input_tokens: 3, output_tokens: 4, total_tokens: 7 } },
  },
]);

const kinds = projected.records.map((record) => record.kind);
assert.deepEqual(kinds.filter((kind) => kind === "user"), ["user"]);
assert.equal(projected.records.filter((record) => record.kind === "assistant").length, 1);
assert.equal(projected.records.find((record) => record.kind === "assistant").output, "Hello");
assert.equal(projected.records.find((record) => record.kind === "thinking").thinking, "think more");
const tool = projected.records.find((record) => record.callId === "c1");
assert.equal(tool.output, "ok");
assert.equal(tool.status, "done");
assert.equal(projected.records.find((record) => record.callId === "c2").kind, "subtool");
const assistant = projected.records.find((record) => record.kind === "assistant");
assert.equal(assistant.tokens.total, 7);
assert.equal(assistant.ttftMs, 100);
assert.equal(formatDurationMs(assistant.ttftMs), "100 ms");

const runtime = projectRuntimeEvents([
  { sequence: 11, kind: "task", name: "accepted", subject_id: "task-a", attributes: { protocol: "demo" } },
  { sequence: 12, kind: "task", name: "completed", subject_id: "task-b", attributes: {} },
], { focusSubjectId: "task-a" });
assert.equal(runtime.records.length, 1);
assert.equal(runtime.records[0].kind, "task");
assert.equal(runtime.records[0].durationMs, null);

const untimedAgent = projectAgentEvents([
  { sequence: 1, meta: {}, event: { type: "user_message", turn_id: "t", content: "x" } },
]);
assert.equal(untimedAgent.records[0].durationMs, null);
assert.equal(untimedAgent.records[0].ttftMs, null);
assert.deepEqual(assistant.raw, []);
assert.ok(JSON.stringify(projected).length < 8_000);

const page = asEventPage({
  events: [{ sequence: 4 }, { sequence: 5 }],
  next_sequence: 5,
  truncated: true,
  lost: 1,
});
assert.equal(page.events.length, 2);
assert.equal(page.truncated, true);
assert.equal(page.lost, 1);

const windowed = trimWindow(
  Array.from({ length: 8 }, (_, index) => ({ sequence: index + 1 })),
  3,
  "older",
);
assert.deepEqual(windowed.map((item) => item.sequence), [1, 2, 3]);

const huge = projectAgentEvents([
  {
    sequence: 1,
    meta: {},
    event: { type: "model_delta", turn_id: "t", text: "x".repeat(5_000) },
  },
]);
assert.ok(huge.records[0].output.length <= 400);

console.log("ok");
