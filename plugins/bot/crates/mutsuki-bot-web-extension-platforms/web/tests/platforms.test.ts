import { createRegistry, type SlotRegistration } from "@mutsuki/web-sdk";
import { createFakeBridge, createTestExtensionContext } from "@mutsuki/web-sdk/testing";
import { flushPromises } from "@vue/test-utils";
import { afterEach, describe, expect, test, vi } from "vitest";
import { defineComponent, h } from "vue";

import AccountsPage from "../src/AccountsPage.vue";
import { renderField } from "../src/fields";
import extension from "../src/index";
import type { PlatformAccountView, PlatformAdapterView } from "../src/types";
import { mountPage } from "./mount";

afterEach(() => {
  document.body.innerHTML = "";
  vi.useRealTimers();
});

const qq: PlatformAdapterView = {
  adapter_id: "mutsuki.plugin.bot.adapter-qqbot",
  platform: "qqbot",
  label: "QQ",
  kind: "messaging",
  config_provider_id: "mutsuki.plugin.bot.adapter-qqbot",
  account_fields: [
    { key: "app_id", label: "AppID", format: "mono" },
    { key: "reconnect_count", label: "重连次数", format: "count" },
    { key: "credential", label: "凭据", format: "secret_status" },
  ],
  account_actions: [
    { id: "health_check", label: "健康检查", danger: false, capability: "bot.platforms.write" },
    { id: "disable", label: "停用", danger: true, capability: "bot.platforms.write" },
  ],
  detail_panels: [],
};
const bilibili: PlatformAdapterView = {
  adapter_id: "mutsuki.plugin.bot.bilibili",
  platform: "bilibili",
  label: "B 站",
  kind: "service_account",
  account_fields: [{ key: "push_wired", label: "推送已接入流程", format: "status" }],
  account_actions: [],
  detail_panels: ["platform.detail:bilibili"],
};
const account = (overrides: Partial<PlatformAccountView>): PlatformAccountView => ({
  adapter_id: qq.adapter_id,
  account_id: "main",
  display_name: "主账号",
  enabled: true,
  health: "ok",
  connection_state: "identified",
  fields: { app_id: "1024", reconnect_count: 2, credential: "configured" },
  ...overrides,
});

function bridgeWith(accounts: PlatformAccountView[]) {
  return createFakeBridge({
    handlers: {
      "platforms.adapters.list": () => [qq, bilibili],
      "platforms.accounts.list": () => accounts,
      "platforms.accounts.action": () => ({ data: null }),
    },
  });
}

describe("field formats", () => {
  const t = (key: string) => ({ "secret.configured": "已配置", "secret.missing": "未配置", yes: "是", no: "否", unknown: "—" })[key] ?? key;
  test("render by declared format", () => {
    expect(renderField("mono", "1024", t)).toEqual({ text: "1024", mono: true });
    expect(renderField("count", 3, t).text).toBe("3");
    expect(renderField("secret_status", "missing", t)).toEqual({ text: "未配置", tone: "err" });
    expect(renderField("status", true, t)).toEqual({ text: "是", tone: "ok" });
    expect(renderField("since", 1_000, t, 66_000).text).toBe("1 分 5 秒");
    expect(renderField("text", null, t).text).toBe("—");
  });
});

describe("accounts page", () => {
  test("groups accounts by adapter and shows descriptor fields", async () => {
    const bridge = bridgeWith([
      account({}),
      account({ adapter_id: bilibili.adapter_id, account_id: "default", display_name: "B 站账号", health: "degraded", fields: { push_wired: false } }),
    ]);
    const { wrapper } = mountPage(AccountsPage, { bridge, props: { slots: createRegistry() } });
    await flushPromises();
    expect(wrapper.findAll(".platforms-group h3").map((node) => node.text())).toEqual(["QQ", "B 站"]);
    const detail = wrapper.find(".mk-split__pane:last-child");
    expect(detail.text()).toContain("主账号");
    expect(detail.text()).toContain("1024");
    expect(detail.text()).toContain("已配置");
    expect(detail.find('a[href="#/plugins/mutsuki.plugin.bot.adapter-qqbot"]').exists()).toBe(true);
    await wrapper.findAll(".platforms-account")[1].trigger("click");
    expect(wrapper.find(".mk-split__pane:last-child").text()).toContain("否");
    wrapper.unmount();
  });

  test("danger actions need confirmation; actions refresh the list", async () => {
    const bridge = bridgeWith([account({})]);
    const { wrapper } = mountPage(AccountsPage, { bridge, props: { slots: createRegistry() } });
    await flushPromises();
    const button = (label: string) => wrapper.findAll("button").find((item) => item.text() === label)!;
    await button("健康检查").trigger("click");
    await flushPromises();
    const actions = () => bridge.calls.filter((call) => call.method === "accounts.action");
    expect(actions()[0]).toMatchObject({
      operation: "write",
      params: { adapter_id: qq.adapter_id, account_id: "main", action: "health_check" },
    });
    expect(document.querySelector(".mk-toast--ok")?.textContent).toContain("健康检查");

    await button("停用").trigger("click");
    await flushPromises();
    expect(document.querySelector('[role="alertdialog"]')).not.toBeNull();
    [...document.querySelectorAll<HTMLButtonElement>(".mk-dialog button")].find((item) => item.textContent?.trim() === "取消")!.click();
    await flushPromises();
    expect(actions()).toHaveLength(1);
    expect(bridge.calls.filter((call) => call.method === "accounts.list").length).toBeGreaterThanOrEqual(2);
    wrapper.unmount();
  });

  test("actions are disabled without the write capability", async () => {
    const { wrapper } = mountPage(AccountsPage, {
      bridge: bridgeWith([account({})]),
      props: { slots: createRegistry() },
      capabilities: ["bot.platforms.read"],
    });
    await flushPromises();
    const health = wrapper.findAll("button").find((item) => item.text() === "健康检查")!;
    expect(health.attributes("disabled")).toBeDefined();
    wrapper.unmount();
  });

  test("detail panels named by the adapter render with the selected account", async () => {
    const slots = createRegistry<SlotRegistration>();
    slots.register({
      id: "bilibili.login",
      slot: "platform.detail:bilibili",
      component: defineComponent({
        props: { account: { type: Object, required: true }, adapter: { type: Object, required: true } },
        setup: (props) => () => h("p", { class: "panel" }, `panel for ${(props.account as PlatformAccountView).account_id}`),
      }),
    });
    const bridge = bridgeWith([
      account({ adapter_id: bilibili.adapter_id, account_id: "default", display_name: "B 站账号", fields: {} }),
    ]);
    const { wrapper } = mountPage(AccountsPage, { bridge, props: { slots } });
    await flushPromises();
    expect(wrapper.find(".panel").text()).toBe("panel for default");
    wrapper.unmount();
  });

  test("refreshes on platforms.changed and explains an empty console", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    const empty = mountPage(AccountsPage, {
      bridge: createFakeBridge({ handlers: { "platforms.adapters.list": () => [], "platforms.accounts.list": () => [] } }),
      props: { slots: createRegistry() },
    });
    await flushPromises();
    expect(empty.wrapper.text()).toContain("尚未启用任何平台适配器");
    empty.bridge.emit("platforms.changed", {});
    await vi.advanceTimersByTimeAsync(60);
    expect(empty.bridge.calls.filter((call) => call.method === "accounts.list")).toHaveLength(2);
    empty.wrapper.unmount();
  });
});

describe("extension", () => {
  test("registers the accounts page in the platforms activity", async () => {
    const ctx = createTestExtensionContext();
    await extension.setup(ctx);
    expect(ctx.navigation.list()[0]).toMatchObject({ activityId: "platforms", pageId: "platforms.accounts" });
  });
});
