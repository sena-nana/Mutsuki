import { RpcError, type JsonValue } from "@mutsuki/web-sdk";
import { createFakeBridge, createTestExtensionContext } from "@mutsuki/web-sdk/testing";
import { flushPromises } from "@vue/test-utils";
import { afterEach, describe, expect, test, vi } from "vitest";

import type { FlowDocument, NodeDescriptor } from "../src/contract";
import FlowEditorPage from "../src/FlowEditorPage.vue";
import { connect, emptyFlow, filterMenu, nodeMenu } from "../src/flow-model";
import { NODE_HEADER, NODE_WIDTH, PORT_ROW, clientToWorld, edgePath, portAnchor, samePortType } from "../src/geometry";
import extension from "../src/index";
import { mountPage } from "./mount";

afterEach(() => {
  document.body.innerHTML = "";
});

const event = (type_id: string) => ({ type_id, version: 1 });
const catalog: NodeDescriptor[] = [
  {
    node_type_id: "qq.message",
    version: 1,
    title: "QQ 消息",
    category: "QQ",
    role: "source",
    ports: [{ port_id: "out", title: "消息", direction: "output", event_type: event("mutsuki.bot.event.message") }],
    config_schema: {},
  },
  {
    node_type_id: "match.keyword",
    version: 1,
    title: "关键词",
    category: "文本",
    role: "match",
    ports: [
      { port_id: "in", title: "事件", direction: "input", event_type: event("mutsuki.bot.event") },
      { port_id: "hit", title: "命中", direction: "output", event_type: event("mutsuki.bot.event.message") },
    ],
    config_schema: {
      properties: {
        keywords: { type: "array", title: "关键词", items: { type: "string" } },
        exact: { type: "boolean", title: "完全匹配" },
        mode: { type: "string", title: "模式", enum: ["any", "all"], enumTitles: ["任一", "全部"], default: "any" },
      },
      required: ["keywords"],
    },
  },
  {
    node_type_id: "sink.reply",
    version: 1,
    title: "回复",
    category: "输出",
    role: "sink",
    ports: [{ port_id: "in", title: "消息", direction: "input", event_type: event("mutsuki.bot.event.message") }],
    config_schema: { properties: { text: { type: "string", title: "回复内容" } } },
  },
];

const flowWith = (...types: string[]): FlowDocument => ({
  ...emptyFlow(),
  nodes: types.map((type, index) => ({
    node_id: `n${index}`,
    node_type_id: type,
    node_type_version: 1,
    config: {},
    position: { x: index * 300, y: 40 },
  })),
});

const button = (label: string) =>
  [...document.querySelectorAll<HTMLButtonElement>("button")].find((item) => item.textContent?.trim() === label)!;

describe("geometry", () => {
  test("port types match by id and version, with event subtypes into the generic input", () => {
    expect(samePortType(event("mutsuki.bot.event"), event("mutsuki.bot.event"))).toBe(true);
    expect(samePortType(event("mutsuki.bot.event.message"), event("mutsuki.bot.event"))).toBe(true);
    expect(samePortType(event("mutsuki.bot.event"), event("mutsuki.bot.flow.error"))).toBe(false);
    expect(samePortType(null, event("mutsuki.bot.event"))).toBe(false);
    expect(samePortType({ type_id: "a", version: 1 }, { type_id: "a", version: 2 })).toBe(false);
  });

  test("anchors, wires and client coordinates", () => {
    const start = portAnchor({ x: 40, y: 80 }, "output", 0);
    const end = portAnchor({ x: 320, y: 160 }, "input", 1);
    expect(start).toEqual({ x: 40 + NODE_WIDTH, y: 80 + NODE_HEADER });
    expect(end).toEqual({ x: 320, y: 160 + NODE_HEADER + PORT_ROW });
    expect(edgePath(start, end)).toMatch(/^M 260 151 C .*, 320 259$/);
    expect(clientToWorld({ left: 100, top: 80 }, { x: 0, y: 0, scale: 1.25 }, 150, 100)).toEqual({ x: 40, y: 16 });
  });
});

describe("flow model", () => {
  test("connect refuses incompatible, duplicate and self edges", () => {
    const flow = flowWith("qq.message", "match.keyword", "sink.reply");
    expect(connect(flow, catalog, { nodeId: "n0", portId: "out" }, { nodeId: "n1", portId: "in" }).ok).toBe(true);
    expect(connect(flow, catalog, { nodeId: "n0", portId: "out" }, { nodeId: "n1", portId: "in" })).toEqual({ ok: false, reason: "duplicate" });
    expect(connect(flow, catalog, { nodeId: "n1", portId: "error" }, { nodeId: "n2", portId: "in" })).toEqual({ ok: false, reason: "incompatible" });
    expect(connect(flow, catalog, { nodeId: "n1", portId: "hit" }, { nodeId: "n1", portId: "in" })).toEqual({ ok: false, reason: "self" });
    expect(flow.edges).toHaveLength(1);
  });

  test("menu groups by role and category; search keeps matching headers only", () => {
    const entries = nodeMenu(catalog, (role) => ({ source: "来源", match: "匹配", processor: "处理", sink: "输出" })[role]);
    expect(entries.filter((entry) => entry.header).map((entry) => entry.label)).toEqual(["来源 · QQ", "匹配 · 文本", "输出 · 输出"]);
    expect(filterMenu(entries, "回复").map((entry) => entry.label)).toEqual(["输出 · 输出", "回复"]);
    expect(filterMenu(entries, "match.keyword").map((entry) => entry.label)).toEqual(["匹配 · 文本", "关键词"]);
  });
});

describe("editor page", () => {
  function bridgeFor(flow: FlowDocument, handlers: Record<string, (params: JsonValue) => unknown> = {}) {
    return createFakeBridge({
      handlers: {
        "bot-flow-editor.catalog.read": () => catalog,
        "bot-flow-editor.snapshot.read": () => ({ revision: 7, flow }),
        "bot-flow-editor.validate": () => ({ valid: true, issues: [] }),
        ...handlers,
      },
    });
  }

  test("adds a node from the searchable menu, wires it from the inspector and saves at the read revision", async () => {
    const apply = vi.fn(() => ({}));
    const bridge = bridgeFor(flowWith("qq.message"), { "bot-flow-editor.apply": apply });
    const { wrapper } = mountPage(FlowEditorPage, { bridge });
    await flushPromises();
    expect(wrapper.findAll("[data-node]")).toHaveLength(1);
    expect(wrapper.text()).toContain("已上线");

    button("添加节点").click();
    await flushPromises();
    const search = document.querySelector<HTMLInputElement>(".ctx-menu input")!;
    search.value = "关键";
    search.dispatchEvent(new Event("input"));
    await flushPromises();
    search.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await flushPromises();
    expect(document.querySelector(".ctx-menu")).toBeNull();
    expect(wrapper.findAll("[data-node]")).toHaveLength(2);
    expect(wrapper.text()).toContain("未保存");

    // The new node is selected: its schema renders, and its default config is applied.
    const inspector = wrapper.find(".flow-inspector");
    expect(inspector.text()).toContain("关键词");
    expect((inspector.find("select").element as HTMLSelectElement).value).toBe("any");
    await inspector.find("form input").setValue("你好");
    await inspector.find("form").trigger("submit");
    expect(inspector.text()).toContain("你好");

    // Wire the source's output to the new node from the source's inspector.
    await wrapper.find('[data-node="n0"]').trigger("pointerdown", { button: 0 });
    await wrapper.find('[data-node="n0"]').trigger("pointerup", { button: 0 });
    const target = wrapper.find(".flow-inspector select");
    const option = target.findAll("option").find((item) => item.text() === "关键词 · 事件")!;
    await target.setValue(option.element.value);
    expect(wrapper.findAll("[data-edge]")).toHaveLength(1);

    button("保存").click();
    await flushPromises();
    expect(apply).toHaveBeenCalledOnce();
    const [params] = apply.mock.calls[0] as unknown as [{ expected_revision: number; flow: FlowDocument }];
    expect(params.expected_revision).toBe(7);
    expect(params.flow.nodes[1].config).toEqual({ keywords: ["你好"], exact: false, mode: "any" });
    expect(params.flow.edges[0]).toMatchObject({ from_node_id: "n0", from_port_id: "out", to_port_id: "in", kind: "event" });
    wrapper.unmount();
  });

  test("validation issues block saving and select their node", async () => {
    const apply = vi.fn();
    const bridge = bridgeFor(flowWith("qq.message", "match.keyword"), {
      "bot-flow-editor.validate": () => ({ valid: false, issues: [{ code: "required", message: "关键词不能为空", severity: "error", node_id: "n1" }] }),
      "bot-flow-editor.apply": apply,
    });
    const { wrapper } = mountPage(FlowEditorPage, { bridge });
    await flushPromises();
    wrapper.find('[data-node="n1"]').element.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter" }));
    await flushPromises();
    await wrapper.find('[role="switch"]').trigger("click");
    button("保存").click();
    await flushPromises();
    expect(apply).not.toHaveBeenCalled();
    expect(wrapper.find('[data-node="n1"]').classes()).toContain("is-invalid");
    button("关键词不能为空").click();
    await flushPromises();
    expect(wrapper.find('[data-node="n1"]').classes()).toContain("is-selected");
    wrapper.unmount();
  });

  test("a revision conflict keeps the edits until reload; other failures show their code", async () => {
    let failure = new RpcError("config.revision_conflict", "revision conflict: expected 7, current 8");
    const bridge = bridgeFor(flowWith("qq.message", "match.keyword"), {
      "bot-flow-editor.apply": () => {
        throw failure;
      },
    });
    const { wrapper } = mountPage(FlowEditorPage, { bridge });
    await flushPromises();
    wrapper.find('[data-node="n1"]').element.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter" }));
    await flushPromises();
    wrapper.find(".lilia-node-editor").element.dispatchEvent(new KeyboardEvent("keydown", { key: "Delete", bubbles: true }));
    await flushPromises();
    expect(wrapper.findAll("[data-node]")).toHaveLength(1);

    button("保存").click();
    await flushPromises();
    expect(wrapper.text()).toContain("流程已在别处更新");
    expect(button("保存").disabled).toBe(true);
    expect(wrapper.findAll("[data-node]")).toHaveLength(1);

    failure = new RpcError("config.persistence_failed", "disk full");
    button("重新加载").click();
    await flushPromises();
    expect(wrapper.findAll("[data-node]")).toHaveLength(2);
    wrapper.find('[data-node="n1"]').element.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter" }));
    await flushPromises();
    await wrapper.find('[role="switch"]').trigger("click");
    button("保存").click();
    await flushPromises();
    expect(wrapper.find('[role="alert"]').text()).toContain("disk full（config.persistence_failed）");
    wrapper.unmount();
  });
});

test("registers the editor in the automation activity", async () => {
  const ctx = createTestExtensionContext();
  await extension.setup(ctx);
  expect(extension.id).toBe("mutsuki.bot.flow");
  expect(ctx.navigation.list().map((item) => [item.activityId, item.pageId])).toEqual([["automation", "bot-flow.page"]]);
});
