import { RpcError, createRegistry, type JsonValue, type NavigationRegistration, type PageRegistration } from "@mutsuki/web-sdk";
import { createFakeBridge, createTestExtensionContext } from "@mutsuki/web-sdk/testing";
import { flushPromises } from "@vue/test-utils";
import { afterEach, describe, expect, test, vi } from "vitest";
import { defineComponent, h } from "vue";

import ConfigEditor from "../src/ConfigEditor.vue";
import { BUILTIN_FORMATS, BUILTIN_KINDS } from "../src/fields";
import extension from "../src/index";
import PluginHub from "../src/PluginHub.vue";
import { createFieldRenderers } from "../src/renderers";
import { candidateOf, draftOf, type ConfigDescriptor, type ConfigNode, type WireValue } from "../src/schema";
import { mountPage } from "./mount";

afterEach(() => {
  document.body.innerHTML = "";
});

const renderers = createFieldRenderers(BUILTIN_KINDS, BUILTIN_FORMATS);
const text = (value: string) => ({ default: value });
const node = (key: string, value_type: ConfigNode["value_type"], extra: Partial<ConfigNode> = {}): ConfigNode => ({
  key,
  value_type,
  title: text(key),
  ...extra,
});
const options = (...values: string[]) => ({ kind: "enum" as const, options: values.map((value) => ({ value, label: text(`选项 ${value}`) })) });

/** Shaped like the product document: a flag, a hidden map and the optional distribution section. */
const schema: ConfigDescriptor = {
  provider_id: "mutsuki.product",
  title: text("工作区"),
  scopes: ["mutsuki.global"],
  root: node("product", { kind: "object" }, {
    children: [
      node("workspace_enabled", { kind: "bool" }, { title: text("启用"), restart_policy: "application_restart" }),
      node(
        "runtime_plugins",
        { kind: "map", key_strategy: "free_string", value: { kind: "object" } },
        { visibility: { op: "literal", value: { type: "bool", value: false } } },
      ),
      node("token", { kind: "secret" }, { title: text("令牌") }),
      node("distribution", { kind: "object" }, {
        title: text("分布式部署"),
        children: [
          node("mode", options("disabled", "clustered"), { title: text("模式") }),
          node("deployment", { kind: "string" }, { title: text("部署文件") }),
          node("fallback", options("reject", "local_degraded"), { title: text("回退") }),
        ],
      }),
    ],
  }),
};

const snapshot = (revision: number, distribution?: Record<string, WireValue>) => ({
  revision,
  value: {
    type: "object",
    value: {
      workspace_enabled: { type: "bool", value: true },
      runtime_plugins: { type: "object", value: { qq: { type: "object", value: { enabled: { type: "bool", value: true } } } } },
      token: { type: "secret", value: { state: "configured" } },
      ...(distribution ? { distribution: { type: "object", value: distribution } } : {}),
    },
  } satisfies WireValue,
});

const button = (label: string) =>
  [...document.querySelectorAll<HTMLButtonElement>("button")].find((item) => item.textContent?.trim() === label)!;

function editorBridge(handlers: Record<string, (params: JsonValue) => unknown> = {}) {
  return createFakeBridge({
    handlers: {
      "config.schema.get": () => schema,
      "config.snapshot.read": () => snapshot(3),
      "config.validate": () => ({ ok: true, issues: [] }),
      ...handlers,
    },
  });
}

describe("schema", () => {
  test("candidate keeps hidden values, omits absent sections and never echoes read secrets", () => {
    const draft = draftOf(snapshot(1));
    expect(candidateOf(schema, draft)).toEqual({
      type: "object",
      value: {
        workspace_enabled: { type: "bool", value: true },
        runtime_plugins: { type: "object", value: { qq: { type: "object", value: { enabled: { type: "bool", value: true } } } } },
        token: { type: "secret", value: { state: "keep" } },
      },
    });
    draft.token = { state: "set", value: "s3cr3t" };
    draft.distribution = { mode: "clustered", deployment: "" };
    const value = candidateOf(schema, draft) as { value: Record<string, WireValue> };
    expect(value.value.token).toEqual({ type: "secret", value: { state: "set", value: "s3cr3t" } });
    // An unset enum stays absent instead of becoming an invalid empty string.
    expect(value.value.distribution).toEqual({
      type: "object",
      value: { mode: { type: "string", value: "clustered" }, deployment: { type: "string", value: "" } },
    });
  });
});

describe("config editor", () => {
  test("renders enums inside an optional nested section and applies against the read revision", async () => {
    const apply = vi.fn(() => ({ revision: 4, applied: true, actions: ["persisted"], pending_actions: [] }));
    const bridge = editorBridge({ "config.apply": apply });
    const { wrapper } = mountPage(ConfigEditor, { bridge, props: { providerId: "mutsuki.product", renderers } });
    await flushPromises();

    expect(wrapper.text()).toContain("分布式部署");
    expect(wrapper.text()).toContain("未配置此部分");
    expect(wrapper.text()).not.toContain("runtime_plugins");
    expect(wrapper.text()).toContain("令牌");
    expect(wrapper.text()).toContain("已配置");

    await wrapper.findAll("button").find((item) => item.text() === "配置此部分")!.trigger("click");
    const select = wrapper.find("fieldset select");
    expect(select.findAll("option").map((option) => option.text())).toEqual(["未设置", "选项 disabled", "选项 clustered"]);
    await select.setValue("clustered");
    expect(wrapper.text()).toContain("有未应用的修改");

    button("应用").click();
    await flushPromises();
    expect(apply).toHaveBeenCalledOnce();
    const params = apply.mock.calls[0] as unknown as [{ request: { expected_revision: number; candidate: { value: Record<string, unknown> } } }];
    expect(params[0].request.expected_revision).toBe(3);
    expect(params[0].request.candidate.value.distribution).toEqual({
      type: "object",
      value: { mode: { type: "string", value: "clustered" }, deployment: { type: "string", value: "" } },
    });
    expect(bridge.calls.filter((call) => call.method === "snapshot.read")).toHaveLength(2);
    wrapper.unmount();
  });

  test("validation issues stop the apply and land on their field", async () => {
    const apply = vi.fn();
    const bridge = editorBridge({
      "config.validate": () => ({
        ok: false,
        issues: [{ path: ["distribution", "mode"], code: "constraint_failed", severity: "error", message: { default: "invalid", zh_cn: "模式无效" } }],
      }),
      "config.apply": apply,
    });
    const { wrapper } = mountPage(ConfigEditor, { bridge, props: { providerId: "mutsuki.product", renderers } });
    await flushPromises();
    await wrapper.findAll("button").find((item) => item.text() === "配置此部分")!.trigger("click");
    button("应用").click();
    await flushPromises();
    expect(apply).not.toHaveBeenCalled();
    expect(wrapper.text()).toContain("有 1 处需要修改");
    expect(wrapper.find("fieldset .mk-field__error").text()).toBe("模式无效");
    wrapper.unmount();
  });

  test("a revision conflict keeps the draft until the user reloads", async () => {
    let revision = 3;
    const bridge = editorBridge({
      "config.snapshot.read": () => snapshot(revision),
      "config.apply": () => {
        throw new RpcError("config.revision_conflict", "revision conflict: expected 3, current 5");
      },
    });
    const { wrapper } = mountPage(ConfigEditor, { bridge, props: { providerId: "mutsuki.product", renderers } });
    await flushPromises();
    await wrapper.find('[role="switch"]').trigger("click");
    button("应用").click();
    await flushPromises();
    expect(wrapper.text()).toContain("配置已在别处更新");
    expect(button("应用").disabled).toBe(true);
    expect(wrapper.text()).toContain("有未应用的修改");

    revision = 5;
    button("重新加载").click();
    await flushPromises();
    expect(wrapper.text()).not.toContain("配置已在别处更新");
    expect(wrapper.text()).not.toContain("有未应用的修改");
    wrapper.unmount();
  });

  test("other failures are shown with their code, not swallowed", async () => {
    const bridge = editorBridge({
      "config.apply": () => {
        throw new RpcError("config.persistence_failed", "persistence failed: disk full");
      },
    });
    const { wrapper } = mountPage(ConfigEditor, { bridge, props: { providerId: "mutsuki.product", renderers } });
    await flushPromises();
    await wrapper.find('[role="switch"]').trigger("click");
    button("应用").click();
    await flushPromises();
    expect(wrapper.find('[role="alert"]').text()).toContain("persistence failed: disk full");
    expect(wrapper.find('[role="alert"]').text()).toContain("config.persistence_failed");
    wrapper.unmount();
  });

  test("a remote revision reloads a clean form and flags a dirty one", async () => {
    let revision = 3;
    const bridge = editorBridge({ "config.snapshot.read": () => snapshot(revision) });
    const { wrapper } = mountPage(ConfigEditor, { bridge, props: { providerId: "mutsuki.product", renderers } });
    await flushPromises();
    revision = 4;
    bridge.emit("config.revision_changed", { provider_id: "mutsuki.product", revision: 4 });
    await flushPromises();
    expect(bridge.calls.filter((call) => call.method === "snapshot.read")).toHaveLength(2);
    expect(wrapper.text()).not.toContain("配置已在别处更新");

    await wrapper.find('[role="switch"]').trigger("click");
    bridge.emit("config.revision_changed", { provider_id: "other", revision: 9 });
    bridge.emit("config.revision_changed", { provider_id: "mutsuki.product", revision: 6 });
    await flushPromises();
    expect(wrapper.text()).toContain("当前版本为 6，本页基于 4");
    wrapper.unmount();
  });

  test("a load failure is shown with a retry", async () => {
    let fail = true;
    const bridge = editorBridge({
      "config.schema.get": () => {
        if (fail) throw new RpcError("config.provider_unavailable", "provider unavailable");
        return schema;
      },
    });
    const { wrapper } = mountPage(ConfigEditor, { bridge, props: { providerId: "mutsuki.product", renderers } });
    await flushPromises();
    expect(wrapper.text()).toContain("配置加载失败");
    fail = false;
    button("重试").click();
    await flushPromises();
    expect(wrapper.text()).toContain("分布式部署");
    wrapper.unmount();
  });
});

describe("renderers", () => {
  test("persona card starts from the legacy flat instruction", async () => {
    const persona: ConfigDescriptor = {
      ...schema,
      root: node("agent", { kind: "object" }, {
        children: [
          node("assistant_instruction", { kind: "string", multiline: true }, { visibility: { op: "literal", value: { type: "bool", value: false } } }),
          node("persona", { kind: "object" }, {
            presentation: { format: "persona-card" },
            children: ["persona_id", "system_prompt"].map((key) => node(key, { kind: "string" })),
          }),
        ],
      }),
    };
    const bridge = editorBridge({
      "config.schema.get": () => persona,
      "config.snapshot.read": () => ({
        revision: 1,
        value: { type: "object", value: { assistant_instruction: { type: "string", value: "你是睦月" } } },
      }),
    });
    const { wrapper } = mountPage(ConfigEditor, { bridge, props: { providerId: "agent", renderers } });
    await flushPromises();
    expect((wrapper.find("textarea").element as HTMLTextAreaElement).value).toBe("你是睦月");
    wrapper.unmount();
  });
});

describe("plugin hubs", () => {
  const navigation = [
    { label: "接入", items: [{ provider_id: "mutsuki.product", label: "工作区" }, { provider_id: "plugin.flow" }, { provider_id: "plugin.never" }] },
  ];

  test("registers hubs in the plugins activity; schema-less plugins only when loaded", async () => {
    const bridge = createFakeBridge({
      handlers: {
        "config.navigation.list": () => navigation,
        "config.schema.get": (params) => {
          if ((params as { provider_id: string }).provider_id === "mutsuki.product") return schema;
          throw new RpcError("config.provider_unavailable", "provider unavailable");
        },
        "control.plugin_list": () => ({
          plugins: [
            { plugin_id: "plugin.flow", active_deployment: "in_process" },
            { plugin_id: "plugin.extra", configured: true },
          ],
        }),
      },
    });
    const ctx = createTestExtensionContext(bridge);
    ctx.pages.register({ id: "flow.page", path: "/flows", title: "流程", pluginId: "plugin.flow", component: {} });
    await extension.setup(ctx);
    const nav = ctx.navigation.list().filter((item) => item.activityId === "plugins");
    expect(nav.map((item) => [item.pageId, item.label, item.group, item.disabled])).toEqual([
      ["mutsuki.product", "工作区", "接入", undefined],
      ["plugin.flow", "plugin.flow", "接入", undefined],
      ["plugin.extra", "plugin.extra", "已加载", true],
    ]);
    expect(ctx.slots.list().map((item) => item.slot)).toEqual(["plugin.home"]);
  });

  test("loaded plugins without a schema are enabled only when they contribute pages or cards", async () => {
    const bridge = createFakeBridge({
      handlers: {
        "config.navigation.list": () => [{ label: "运行插件", items: [{ provider_id: "mutsuki.bot.command", label: "命令" }] }],
        "config.schema.get": () => {
          throw new RpcError("config.provider_unavailable", "provider unavailable");
        },
        "control.plugin_list": () => ({
          plugins: [
            { plugin_id: "mutsuki.bot.command", configured: true, active_deployment: "builtin" },
            { plugin_id: "mutsuki.bot.bilibili", configured: true, active_deployment: "builtin" },
            { plugin_id: "mutsuki.bot.sandbox", configured: true, active_deployment: "builtin" },
            { plugin_id: "mutsuki.catalog.only", configured: false, active_deployment: null },
          ],
        }),
      },
    });
    const ctx = createTestExtensionContext(bridge);
    ctx.pages.register({ id: "bilibili.subscriptions", path: "/automation/bilibili", title: "B 站推送", pluginId: "mutsuki.bot.bilibili", component: {} });
    ctx.slots.register({ id: "sandbox.card", slot: "overview.cards", pluginId: "mutsuki.bot.sandbox", component: {} });
    await extension.setup(ctx);
    const nav = Object.fromEntries(ctx.navigation.list().map((item) => [item.pageId, item]));
    expect(nav["mutsuki.bot.command"]).toMatchObject({ label: "命令", group: "运行插件", disabled: true });
    expect(nav["mutsuki.bot.bilibili"]).toMatchObject({ group: "已加载", disabled: undefined });
    expect(nav["mutsuki.bot.sandbox"]).toMatchObject({ group: "已加载", disabled: undefined });
    expect(nav["mutsuki.catalog.only"]).toBeUndefined();
  });

  test("many providers stay within the extension's RPC budget", async () => {
    let inFlight = 0;
    let peak = 0;
    const items = Array.from({ length: 30 }, (_, index) => ({ provider_id: `p${index}` }));
    const bridge = createFakeBridge({
      handlers: {
        "config.navigation.list": () => [{ items }],
        "config.schema.get": async () => {
          peak = Math.max(peak, ++inFlight);
          await new Promise((resolve) => setTimeout(resolve, 1));
          inFlight -= 1;
          return schema;
        },
        "control.plugin_list": () => ({ plugins: [] }),
      },
    });
    const ctx = createTestExtensionContext(bridge);
    await extension.setup(ctx);
    expect(ctx.navigation.list()).toHaveLength(30);
    expect(peak).toBeLessThanOrEqual(8);
  });

  test("provider hubs survive a failing plugin list", async () => {
    const bridge = createFakeBridge({
      handlers: {
        "config.navigation.list": () => [{ items: [{ provider_id: "mutsuki.product", label: "工作区" }] }],
        "config.schema.get": () => schema,
        "control.plugin_list": () => {
          throw new Error("control unavailable");
        },
      },
    });
    const ctx = createTestExtensionContext(bridge);
    await extension.setup(ctx);
    expect(ctx.navigation.list().map((item) => [item.pageId, item.label, item.group])).toEqual([["mutsuki.product", "工作区", undefined]]);
  });

  test("a hub shows the owner's config.form instead of the generated form, and its pages", async () => {
    const slots = createRegistry<{ id: string; slot: string; component: unknown; pluginId?: string }>();
    const OwnerForm = defineComponent({ props: { providerId: String }, setup: (props) => () => h("p", { class: "owner-form" }, props.providerId) });
    slots.register({ id: "owner.form", slot: "config.form", pluginId: "plugin.flow", component: OwnerForm });
    const pages = createRegistry<PageRegistration>();
    const navigationRegistry = createRegistry<NavigationRegistration>();
    pages.register({ id: "flow.page", path: "/flows", title: "流程", pluginId: "plugin.flow", component: {} });
    navigationRegistry.register({ id: "flow.nav", activityId: "automation", pageId: "flow.page", label: "流程编排" });
    const bridge = createFakeBridge({
      handlers: { "control.plugin_list": () => ({ plugins: [{ plugin_id: "plugin.flow", active_deployment: "in_process", candidates: [{ deployment: "in_process", version: "0.2.0" }] }] }), "config.metrics": () => ({}) },
    });
    const { wrapper } = mountPage(PluginHub, {
      bridge,
      props: { pluginId: "plugin.flow", hasSchema: true, slots, registries: { pages, navigation: navigationRegistry }, renderers },
    });
    await flushPromises();
    expect(wrapper.find(".owner-form").text()).toBe("plugin.flow");
    expect(bridge.calls.some((call) => call.method === "schema.get")).toBe(false);
    expect(wrapper.find('a[href="#/automation/flow.page"]').text()).toBe("流程编排");
    expect(wrapper.text()).toContain("版本 0.2.0");
    wrapper.unmount();
  });
});
