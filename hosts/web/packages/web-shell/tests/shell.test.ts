// @vitest-environment happy-dom
import { flushPromises, mount } from "@vue/test-utils";
import { describe, expect, test, vi } from "vitest";
import { defineComponent, h, onMounted, provide, ref } from "vue";

import { createI18n, provideI18n, RpcError } from "@mutsuki/web-sdk";
import { uiMessages } from "@mutsuki/ui";
import ConsoleRoot from "../src/app/ConsoleRoot.vue";
import ExtensionBoundary from "../src/app/ExtensionBoundary.vue";
import { SHELL_CONTEXT } from "../src/app/context";
import { placeNavigation, type ConsoleLayout } from "../src/layout";
import { shellMessages } from "../src/messages";
import { buildShellModel } from "../src/navigation";
import { createShellState, loadExtensions, type WebShellRuntime } from "../src/runtime";

const layout: ConsoleLayout = {
  activities: [
    { id: "home", label: "总览", icon: "home", order: 0, position: "top" },
    { id: "agent", label: "会话与 Agent", icon: "chat", order: 20, position: "top" },
    { id: "data", label: "数据", icon: "data", order: 50, position: "top" },
    { id: "settings", label: "设置", icon: "settings", order: 100, position: "bottom" },
  ],
};

function i18nPlugin() {
  const i18n = createI18n().extend("ui", uiMessages).extend("shell", shellMessages);
  return { install: (app: Parameters<typeof provideI18n>[0]) => provideI18n(app, i18n) };
}

describe("layout placement", () => {
  test("keeps navigation filed under layout activities and reports the rest", () => {
    const nav = (id: string, activityId: string) => ({ id, activityId, pageId: `${id}.page`, label: id });
    const { placed, failures } = placeNavigation(
      layout,
      [nav("overview.nav", "home"), nav("sessions.nav", "agent"), nav("old.nav", "bot"), nav("x.nav", "nowhere")],
      (item) => `owner-of-${item.id}`,
    );
    expect(placed.map((item) => [item.id, item.activityId])).toEqual([
      ["overview.nav", "home"],
      ["sessions.nav", "agent"],
    ]);
    expect(failures).toEqual([
      expect.objectContaining({ extensionId: "owner-of-old.nav", stage: "layout" }),
      expect.objectContaining({ extensionId: "owner-of-x.nav", stage: "layout" }),
    ]);
  });

  test("an extension that still registers activities fails to load; the layout is untouched", async () => {
    const state = createShellState();
    for (const activity of layout.activities) state.activities.register(activity);
    (globalThis as Record<string, unknown>).__layoutExtension = (ctx: Record<string, { register(item: object): unknown }>) =>
      ctx.activities.register({ id: "automation", label: "自动化", icon: "flow" });
    await loadExtensions(
      state,
      [{ id: "v1", url: "data:text/javascript,export default {setup(ctx){return globalThis.__layoutExtension(ctx)}}" }],
      () => ({
        pages: state.pages,
        navigation: state.navigation,
        slots: state.slots,
        commands: state.commands,
        rpc: {} as never,
        events: { subscribe: () => ({ dispose() {} }) },
      }),
    );
    expect(state.extensions).toEqual([]);
    expect(state.activities.list().map((item) => item.id)).toEqual(["home", "agent", "data", "settings"]);
    expect(state.failures).toEqual([expect.objectContaining({ extensionId: "v1", stage: "load" })]);
  });

  test("model hides empty activities, filters capabilities and prefers the home page", () => {
    const state = createShellState();
    const page = (id: string, requiredCapability?: string) =>
      state.pages.register({ id, path: `/${id}`, title: id, component: { render: () => null }, requiredCapability });
    page("overview");
    page("db", "data.read");
    page("sessions");
    state.navigation.register({ id: "overview.nav", activityId: "home", pageId: "overview", label: "总览" });
    state.navigation.register({ id: "database.nav", activityId: "data", pageId: "db", label: "数据库" });
    state.navigation.register({ id: "sessions.nav", activityId: "agent", pageId: "sessions", label: "会话" });
    state.owners.set("database.nav", "database");
    const model = buildShellModel(state, layout, (cap) => cap !== "data.read", "sessions");
    expect(model.activities.map((item) => item.id)).toEqual(["home", "agent"]);
    expect(model.pages.has("db")).toBe(false);
    expect(model.firstRoute()).toEqual({ activityId: "agent", pageId: "sessions" });
    expect(model.routeFor("agent", "mutsuki.plugin.bot.flow")).toBe("/agent/mutsuki.plugin.bot.flow");
  });
});

describe("the extension boundary", () => {
  function shellContext(reportFailure = vi.fn()) {
    return { reportFailure, failures: ref([]) } as never;
  }

  test("a failing extension page renders an error state and is reported", async () => {
    const reportFailure = vi.fn();
    const Broken = defineComponent({
      setup() {
        onMounted(() => {
          throw new RpcError("internal", "page exploded");
        });
        return () => h("p", "page");
      },
    });
    const Host = defineComponent({
      setup() {
        provide(SHELL_CONTEXT, shellContext(reportFailure));
        return () => h("div", [h(ExtensionBoundary, { extensionId: "control" }, () => h(Broken)), h("p", "shell alive")]);
      },
    });
    const wrapper = mount(Host, { global: { plugins: [i18nPlugin()] } });
    await flushPromises();
    expect(wrapper.find('[role="alert"]').text()).toContain("control");
    expect(wrapper.text()).toContain("shell alive");
    expect(reportFailure).toHaveBeenCalledWith(
      expect.objectContaining({ extensionId: "control", stage: "render", message: "page exploded" }),
    );
  });
});

describe("ConsoleRoot", () => {
  test("shows the login on a rejected session, then starts after a good token", async () => {
    const runtime = { bridge: { onStateChange: () => ({ dispose() {} }) } } as unknown as WebShellRuntime;
    const start = vi
      .fn<() => Promise<WebShellRuntime>>()
      .mockRejectedValueOnce(new RpcError("auth_failed", "session not authenticated"))
      .mockResolvedValueOnce(runtime);
    const login = vi.fn(async (token: string) => token === "right");
    const wrapper = mount(ConsoleRoot, {
      props: { brand: "Mutsuki", start, login },
      slots: { default: () => h("p", { class: "ready" }, "shell") },
      global: { plugins: [i18nPlugin()] },
    });
    await flushPromises();
    expect(wrapper.find("form").exists()).toBe(true);
    expect(wrapper.find('[role="alert"]').text()).toBe("");

    await wrapper.find("input").setValue("wrong");
    await wrapper.find("form").trigger("submit");
    await flushPromises();
    expect(wrapper.find('[role="alert"]').text()).toBe("口令不正确，请重试。");
    expect((wrapper.find("input").element as HTMLInputElement).value).toBe("");

    await wrapper.find("input").setValue("right");
    await wrapper.find("form").trigger("submit");
    await flushPromises();
    expect(login).toHaveBeenLastCalledWith("right");
    expect(wrapper.find(".ready").exists()).toBe(true);
  });

  test("a non-auth connection failure keeps the login with a load error", async () => {
    const start = vi.fn<() => Promise<WebShellRuntime>>().mockRejectedValue(new RpcError("disconnected", "down"));
    const wrapper = mount(ConsoleRoot, {
      props: { brand: "Mutsuki", start, login: async () => true },
      global: { plugins: [i18nPlugin()] },
    });
    await flushPromises();
    expect(wrapper.find('[role="alert"]').text()).toBe("控制台加载失败，请重试。");
  });
});
