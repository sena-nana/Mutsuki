import type { JsonValue } from "@mutsuki/web-sdk";
import { createFakeBridge, createTestExtensionContext } from "@mutsuki/web-sdk/testing";
import { flushPromises } from "@vue/test-utils";
import { afterEach, describe, expect, test, vi } from "vitest";


import extension from "../src/index";
import LifecyclePage from "../src/pages/LifecyclePage.vue";
import LogsPage from "../src/pages/LogsPage.vue";
import PluginsPage from "../src/pages/PluginsPage.vue";
import RunnersPage from "../src/pages/RunnersPage.vue";
import TasksPage from "../src/pages/TasksPage.vue";
import { mergeBySequence, projectRuntimeEvents, trimWindow } from "../src/runtime-events";
import { mountPage } from "./mount";

afterEach(() => {
  document.body.innerHTML = "";
  vi.useRealTimers();
});

const dialogButton = (label: string) =>
  [...document.body.querySelectorAll<HTMLButtonElement>(".mk-dialog button")].find(
    (button) => button.textContent?.trim() === label,
  )!;
const pageButton = (wrapper: { findAll(selector: string): Array<{ text(): string; trigger(event: string): Promise<void> }> }, label: string) =>
  wrapper.findAll("button").find((button) => button.text() === label)!;
const registries = () => {
  const ctx = createTestExtensionContext();
  return { pages: ctx.pages, navigation: ctx.navigation };
};

describe("runtime event projection", () => {
  test("focuses one task, marks errors and keeps runtime-wide events", () => {
    const { records, groups } = projectRuntimeEvents(
      [
        { sequence: 1, subject_id: "t1", kind: "task", name: "submitted", attributes: { protocol: "p" } },
        { sequence: 2, subject_id: "t2", kind: "task", name: "submitted" },
        { sequence: 3, kind: "reload", name: "generation" },
        { sequence: 4, subject_id: "t1", kind: "runner", name: "failed", error: { message: "boom" } },
      ],
      { focusSubjectId: "t1" },
    );
    expect(records.map((record) => [record.label, record.kind, record.groupId])).toEqual([
      ["submitted", "task", "t1"],
      ["generation", "system", "runtime"],
      ["failed", "error", "t1"],
    ]);
    expect(records[0].preview).toBe("submitted · protocol=p");
    expect(records[2].preview).toBe("failed: boom");
    expect(groups.map((group) => group.label)).toEqual(["任务 t1", "运行时"]);
  });

  test("merges pages by sequence and bounds the window", () => {
    const merged = mergeBySequence([{ sequence: 3 }, { sequence: 1 }], [{ sequence: 2 }, { sequence: 3, name: "new" }, {}]);
    expect(merged.map((event) => [event.sequence, event.name])).toEqual([[1, undefined], [2, undefined], [3, "new"]]);
    expect(trimWindow([1, 2, 3, 4], 2, "older")).toEqual([1, 2]);
    expect(trimWindow([1, 2, 3, 4], 2)).toEqual([3, 4]);
  });
});

describe("plugins page", () => {
  const pluginList = () => ({
    plugins: [
      {
        plugin_id: "mutsuki.plugin.bot.flow",
        active_deployment: "builtin",
        candidates: [
          { deployment: "builtin", version: "0.1.0", available: true },
          { deployment: "abi", version: "0.1.0", available: false },
        ],
      },
    ],
    diagnostics: [{ plugin_id: "broken.plugin", detail: "invalid manifest version" }],
  });

  test("confirmation gates the deployment write and refreshes after it", async () => {
    const bridge = createFakeBridge({
      handlers: { "control.plugin_list": pluginList, "control.plugin_deployment_set": () => null },
    });
    const { wrapper } = mountPage(PluginsPage, { bridge, props: { registries: registries() } });
    await flushPromises();
    expect(wrapper.text()).toContain("invalid manifest version");
    const prefer = wrapper.findAll("button").filter((button) => button.text() === "设为偏好");
    expect(prefer[1].attributes("disabled")).toBeDefined();

    await prefer[0].trigger("click");
    await flushPromises();
    dialogButton("取消").click();
    await flushPromises();
    expect(bridge.calls.some((call) => call.method === "plugin_deployment_set")).toBe(false);

    await prefer[0].trigger("click");
    await flushPromises();
    dialogButton("确认").click();
    await flushPromises();
    const write = bridge.calls.find((call) => call.method === "plugin_deployment_set")!;
    expect(write).toMatchObject({
      operation: "write",
      params: { plugin_id: "mutsuki.plugin.bot.flow", deployment: "builtin" },
    });
    expect(bridge.calls.filter((call) => call.method === "plugin_list")).toHaveLength(2);
    wrapper.unmount();
  });

  test("only changes to its own domain trigger a refresh", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    const bridge = createFakeBridge({ handlers: { "control.plugin_list": pluginList } });
    const { wrapper } = mountPage(PluginsPage, { bridge, props: { registries: registries() } });
    await flushPromises();
    bridge.emit("control.changed", { revision: 1, domains: ["runners"] });
    await vi.advanceTimersByTimeAsync(60);
    expect(bridge.calls.filter((call) => call.method === "plugin_list")).toHaveLength(1);
    bridge.emit("control.changed", { revision: 2, domains: ["plugins"] });
    bridge.emit("control.changed", { revision: 2, domains: ["plugins"] });
    await vi.advanceTimersByTimeAsync(60);
    await flushPromises();
    expect(bridge.calls.filter((call) => call.method === "plugin_list")).toHaveLength(2);
    wrapper.unmount();
  });
});

describe("runners and lifecycle", () => {
  test("stopping a runner is a danger confirmation; failures become a toast", async () => {
    const bridge = createFakeBridge({
      handlers: {
        "control.runner_list": () => [{ runner_id: "r1", plugin_id: "p", state: "running", pid: 7, restarts: 0 }],
        "control.runner_stop": () => {
          throw new Error("runner busy");
        },
      },
    });
    const { wrapper } = mountPage(RunnersPage, { bridge });
    await flushPromises();
    expect(wrapper.text()).toContain("p · 运行中 · 进程 7 · 重启 0");
    await pageButton(wrapper, "停止").trigger("click");
    await flushPromises();
    expect(document.body.querySelector('[role="alertdialog"]')).not.toBeNull();
    dialogButton("确认").click();
    await flushPromises();
    expect(document.body.querySelector(".mk-toast--err")?.textContent).toContain("runner busy");
    wrapper.unmount();
  });

  test("shutdown requires typing the confirmation word", async () => {
    const bridge = createFakeBridge({ handlers: { "control.service_shutdown": () => null } });
    const { wrapper } = mountPage(LifecyclePage, { bridge });
    await pageButton(wrapper, "关闭服务").trigger("click");
    await flushPromises();
    expect(dialogButton("关闭服务").disabled).toBe(true);
    const input = document.body.querySelector<HTMLInputElement>(".mk-dialog input")!;
    input.value = "关闭";
    input.dispatchEvent(new Event("input"));
    await flushPromises();
    dialogButton("关闭服务").click();
    await flushPromises();
    expect(bridge.calls.map((call) => call.method)).toEqual(["service_shutdown"]);
    expect(document.body.querySelector(".mk-toast--ok")?.textContent).toContain("关闭信号已发送");
    wrapper.unmount();
  });
});

describe("tasks and logs", () => {
  test("selecting a task focuses the timeline on its events", async () => {
    const events = [
      { sequence: 1, subject_id: "t1", kind: "task", name: "t1 submitted" },
      { sequence: 2, subject_id: "t2", kind: "task", name: "t2 submitted" },
    ];
    const bridge = createFakeBridge({
      handlers: {
        "control.task_list": () => [
          { task_id: "t1", protocol_id: "p", status: "running" },
          { task_id: "t2", protocol_id: "p", status: "queued" },
        ],
        "control.task_events_after": (params): JsonValue =>
          (params as { limit: number }).limit === 1
            ? { earliest_available_sequence: 1, latest_sequence: 2, events: [] }
            : { events },
      },
    });
    const { wrapper } = mountPage(TasksPage, { bridge });
    await flushPromises();
    const ledger = () => wrapper.find(".trajectory-ledger").text();
    expect(ledger()).toContain("t1 submitted");
    expect(ledger()).not.toContain("t2 submitted");
    await wrapper.findAll(".task-select")[1].trigger("click");
    await flushPromises();
    expect(ledger()).toContain("t2 submitted");
    expect(ledger()).not.toContain("t1 submitted");

    await wrapper.find(".trajectory-row").trigger("click");
    await flushPromises();
    expect(wrapper.find(".trajectory-inspector").text()).toContain('"sequence": 2');
    wrapper.unmount();
  });

  test("log tail appends by cursor and restarts after rotation", async () => {
    const tails = [
      { cursor: 10, entries: [{ offset: 1, line: "first" }] },
      { cursor: 12, entries: [{ offset: 1, line: "first" }, { offset: 2, line: "second" }] },
      { cursor: 3, entries: [{ offset: 1, line: "rotated" }] },
    ];
    const bridge = createFakeBridge({ handlers: { "control.log_tail": () => tails.shift()! } });
    const { wrapper } = mountPage(LogsPage, { bridge });
    await flushPromises();
    expect(wrapper.find("pre").text()).toBe("first");
    await pageButton(wrapper, "刷新").trigger("click");
    await flushPromises();
    expect(wrapper.find("pre").text()).toBe("first\nsecond");
    expect(bridge.calls[1].params).toEqual({ cursor: 10, lines: 500 });
    await pageButton(wrapper, "刷新").trigger("click");
    await flushPromises();
    expect(wrapper.find("pre").text()).toBe("rotated");
    wrapper.unmount();
  });
});

describe("extension", () => {
  test("registers the plugin list under plugins and six system pages", async () => {
    const ctx = createTestExtensionContext();
    await extension.setup(ctx);
    expect(extension.id).toBe("mutsuki.web.diagnostics");
    expect(ctx.navigation.list().map((item) => item.activityId)).toEqual(["plugins", ...Array(6).fill("system")]);
    expect(ctx.pages.list().map((page) => page.id)).toEqual([
      "control.plugins",
      "control.runners",
      "control.events",
      "control.topology",
      "control.tasks",
      "control.lifecycle",
      "control.logs",
    ]);
  });
});
