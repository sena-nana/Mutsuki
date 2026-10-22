import { RpcError } from "@mutsuki/web-sdk";
import { createFakeBridge, createTestExtensionContext } from "@mutsuki/web-sdk/testing";
import { flushPromises } from "@vue/test-utils";
import { afterEach, describe, expect, test } from "vitest";

import { projectAgentEvents } from "../src/agent-events";
import type { AgentEventEnvelope } from "../src/contract";
import ApprovalsPage from "../src/ApprovalsPage.vue";
import ConnectionsPage from "../src/ConnectionsPage.vue";
import extension from "../src/index";
import SessionsPage from "../src/SessionsPage.vue";
import { mountPage } from "./mount";

afterEach(() => {
  document.body.innerHTML = "";
});

const dialogButton = (label: string) =>
  [...document.querySelectorAll<HTMLButtonElement>(".mk-dialog button")].find((item) => item.textContent?.trim() === label)!;

const envelope = (sequence: number, event: Record<string, unknown> & { type: string }): AgentEventEnvelope =>
  ({
    session_id: "s1",
    sequence,
    meta: { timestamp_unix_ms: 1_000 + sequence * 100 },
    event,
  }) as AgentEventEnvelope;

describe("agent event projection", () => {
  test("merges streams per turn and tool calls by id", () => {
    const { records, groups } = projectAgentEvents([
      envelope(1, { type: "user_message", turn_id: "t1", content: "查天气" }),
      envelope(2, { type: "reasoning_delta", turn_id: "t1", text: "需要" }),
      envelope(3, { type: "tool_call_started", turn_id: "t1", call_id: "c1", name: "weather", input: { city: "上海" } }),
      envelope(4, { type: "tool_call_completed", turn_id: "t1", call_id: "c1", summary: "晴" }),
      envelope(5, { type: "model_delta", turn_id: "t1", text: "今天" }),
      envelope(6, { type: "model_delta", turn_id: "t1", text: "晴" }),
      envelope(7, { type: "usage", turn_id: "t1", usage: { input_tokens: 10, output_tokens: 2, total_tokens: 12 } }),
      envelope(8, { type: "final", turn_id: "t1", summary: "今天晴" }),
      envelope(9, { type: "session_state", status: "active" }),
      envelope(10, { type: "command_exited", turn_id: "t2", command_id: "x", exit_code: 1, summary: "failed" }),
    ]);
    expect(records.map((record) => [record.kind, record.label, record.status])).toEqual([
      ["user", "用户", "done"],
      ["thinking", "思考", "running"],
      ["tool", "weather", "done"],
      ["assistant", "助手", "done"],
      ["command", "命令", "error"],
    ]);
    const assistant = records[3];
    expect(assistant.output).toBe("今天晴");
    expect(assistant.tokens).toEqual({ input: 10, output: 2, total: 12 });
    expect(assistant.ttftMs).toBe(100);
    expect(records[2].input).toContain("上海");
    expect(records[2].sequences).toEqual([3, 4]);
    expect(groups.map((group) => group.label)).toEqual(["第 t1 轮", "第 t2 轮"]);
  });
});

function sessionBridge() {
  const approvals = [
    { request: { session_id: "s1", turn_id: "t1", action_id: "a1", tool: "shell", summary: "运行 ls", version: 4 } },
  ];
  return createFakeBridge({
    handlers: {
      "bot-agent.sessions.list": () => ({
        items: [
          { session_id: "s1", title: "天气", turn_count: 2, message_count: 3, status: "waiting_approval" },
          { session_id: "s2", title: "闲聊", turn_count: 1, message_count: 2, status: "completed" },
        ],
        next_session_id: null,
      }),
      "bot-agent.sessions.get": () => ({
        session_id: "s1",
        title: "天气",
        turn_count: 2,
        next_event_sequence: 2,
        messages: [
          { role: "user", content: "上海天气？" },
          { role: "assistant", content: "晴" },
        ],
      }),
      "bot-agent.sessions.state": () => ({
        session_id: "s1",
        version: 4,
        status: "waiting_approval",
        turns: [{ turn_id: "t1", status: "running_tools" }],
        pending_approvals: approvals,
        pending_interactions: [{ session_id: "s1", turn_id: "t1", version: 4, interaction_id: "i1", prompt: "哪个城市？" }],
      }),
      "bot-agent.sessions.events": () => ({
        events: [envelope(1, { type: "user_message", turn_id: "t1", content: "上海天气？" })],
        next_sequence: 2,
        lost: 0,
        truncated: false,
      }),
      "bot-agent.sessions.approve": () => ({}),
      "bot-agent.sessions.reject": () => ({}),
      "bot-agent.sessions.interact": () => ({}),
      "bot-agent.sessions.cancel_turn": () => ({}),
    },
  });
}

describe("sessions page", () => {
  test("shows a transcript, answers interactions and approves after confirmation", async () => {
    const bridge = sessionBridge();
    const { wrapper } = mountPage(SessionsPage, { bridge });
    await flushPromises();
    await wrapper.find(".agent-session-item").trigger("click");
    await flushPromises();
    expect(wrapper.find(".agent-transcript").text()).toContain("上海天气？");
    expect(wrapper.find(".trajectory-ledger").text()).toContain("上海天气？");

    await wrapper.find('input[type="text"]').setValue("上海");
    await wrapper.findAll("button").find((button) => button.text() === "提交回答")!.trigger("click");
    await flushPromises();
    expect(bridge.calls.find((call) => call.method === "sessions.interact")?.params).toEqual({
      resolution: { session_id: "s1", turn_id: "t1", version: 4, interaction_id: "i1", accepted: true, response: { answer: "上海" } },
    });

    await wrapper.findAll("button").find((button) => button.text() === "批准")!.trigger("click");
    await flushPromises();
    dialogButton("确认").click();
    await flushPromises();
    expect(bridge.calls.find((call) => call.method === "sessions.approve")?.params).toMatchObject({
      decision: { action_id: "a1", version: 4, decision: "approved" },
    });

    await wrapper.findAll("button").find((button) => button.text() === "停止当前回复")!.trigger("click");
    await flushPromises();
    dialogButton("确认").click();
    await flushPromises();
    expect(bridge.calls.find((call) => call.method === "sessions.cancel_turn")?.params).toEqual({
      session_id: "s1",
      turn_id: "t1",
      expected_version: 4,
    });
    wrapper.unmount();
  });
});

describe("approvals inbox", () => {
  test("collects pending approvals of waiting sessions only", async () => {
    const bridge = sessionBridge();
    const { wrapper } = mountPage(ApprovalsPage, { bridge });
    await flushPromises();
    expect(bridge.calls.filter((call) => call.method === "sessions.state").map((call) => call.params)).toEqual([{ session_id: "s1" }]);
    const row = wrapper.find("tbody tr");
    expect(row.text()).toContain("天气");
    expect(row.text()).toContain("运行 ls");
    await row.findAll("button").find((button) => button.text() === "拒绝")!.trigger("click");
    await flushPromises();
    expect(bridge.calls.find((call) => call.method === "sessions.reject")?.params).toMatchObject({ decision: { decision: "rejected" } });
    wrapper.unmount();
  });
});

describe("connections page", () => {
  const snapshot = {
    revision: 3,
    connections: [{ connection_id: "local", connector_id: "openai", generation: 1, state: "healthy", enabled_features: [], negotiated_version: 1 }],
  };

  test("reconnects against the snapshot revision and validates config JSON before testing", async () => {
    const bridge = createFakeBridge({
      handlers: {
        "bot-agent.connections.snapshot": () => snapshot,
        "bot-agent.connections.reconnect": () => snapshot.connections[0],
        "bot-agent.connections.test": () => ({ ...snapshot.connections[0], state: "healthy" }),
        "bot-agent.connections.upsert": () => snapshot.connections[0],
        "bot-agent.provider.test": () => ({ ok: true }),
      },
    });
    const { wrapper } = mountPage(ConnectionsPage, { bridge });
    await flushPromises();
    expect(wrapper.find("tbody").text()).toContain("本机模型");
    await wrapper.findAll("button").find((button) => button.text() === "重连")!.trigger("click");
    await flushPromises();
    dialogButton("确认").click();
    await flushPromises();
    expect(bridge.calls.find((call) => call.method === "connections.reconnect")?.params).toEqual({ expected_revision: 3, connection_id: "local" });

    await wrapper.findAll("button").find((button) => button.text() === "编辑")!.trigger("click");
    await flushPromises();
    const textarea = document.querySelector<HTMLTextAreaElement>(".mk-dialog textarea")!;
    textarea.value = "[1]";
    textarea.dispatchEvent(new Event("input"));
    await flushPromises();
    dialogButton("测试连接").click();
    await flushPromises();
    expect(document.querySelector(".mk-dialog .mk-field__error")?.textContent).toBe("配置必须是 JSON 对象");
    expect(bridge.calls.some((call) => call.method === "connections.test")).toBe(false);

    textarea.value = '{"model":"gpt"}';
    textarea.dispatchEvent(new Event("input"));
    await flushPromises();
    dialogButton("测试连接").click();
    await flushPromises();
    expect(bridge.calls.find((call) => call.method === "connections.test")?.params).toEqual({
      config: { connection_id: "local", connector_id: "openai", enabled: true, config: { model: "gpt" } },
    });
    expect(document.querySelector(".mk-toast--info")?.textContent).toContain("运行中");

    dialogButton("保存").click();
    await flushPromises();
    dialogButton("确认").click();
    await flushPromises();
    expect(bridge.calls.find((call) => call.method === "connections.upsert")?.params).toMatchObject({ expected_revision: 3 });

    await wrapper.findAll("button").find((button) => button.text() === "测试本机模型")!.trigger("click");
    await flushPromises();
    expect(bridge.calls.some((call) => call.method === "provider.test")).toBe(true);
    wrapper.unmount();
  });
});

describe("extension", () => {
  test("registers only the surfaces whose services answer", async () => {
    const sessionsOnly = createFakeBridge({
      handlers: { "bot-agent.sessions.list": () => ({ items: [], next_session_id: null }) },
    });
    const ctx = createTestExtensionContext(sessionsOnly);
    await extension.setup(ctx);
    expect(ctx.pages.list().map((page) => page.id)).toEqual(["bot-agent.page", "agent.approvals"]);
    expect(ctx.navigation.list().every((item) => item.activityId === "agent")).toBe(true);
    expect(ctx.slots.list()).toEqual([]);

    const denied = createFakeBridge({
      handlers: {
        "bot-agent.connections.snapshot": () => {
          throw new RpcError("capability_denied", "missing agent.connection.read");
        },
      },
    });
    const deniedCtx = createTestExtensionContext(denied);
    await extension.setup(deniedCtx);
    expect(deniedCtx.pages.list().map((page) => page.id)).toEqual(["agent.connections"]);
  });
});
