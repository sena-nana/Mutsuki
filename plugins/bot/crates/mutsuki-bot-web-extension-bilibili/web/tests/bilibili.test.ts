import type { JsonValue } from "@mutsuki/web-sdk";
import { createFakeBridge, createTestExtensionContext } from "@mutsuki/web-sdk/testing";
import { flushPromises } from "@vue/test-utils";
import { afterEach, describe, expect, test, vi } from "vitest";

import BilibiliAccountPanel from "../src/BilibiliAccountPanel.vue";
import extension from "../src/index";
import SubscriptionsPage from "../src/SubscriptionsPage.vue";
import { mountPage } from "./mount";

afterEach(() => {
  vi.useRealTimers();
  document.body.innerHTML = "";
});

const button = (root: ParentNode, label: string) =>
  [...root.querySelectorAll<HTMLButtonElement>("button")].find((item) => item.textContent?.trim() === label)!;
const dialogButton = (label: string) => button(document.querySelector(".mk-dialog")!, label);

const status = (patch: Record<string, unknown> = {}) => ({
  backend: "local",
  management_enabled: true,
  allow_self_binding: false,
  credential_loaded: false,
  subscription_count: 1,
  ...patch,
});

const subscription = {
  subscription_id: "daily",
  uid: 42,
  notifications: ["live", "video"],
  target: { type: "group", group_id: "1001" },
  outbound_binding: "qq-main",
  paused: false,
  owner_user_id: null,
};

describe("extension", () => {
  test("registers the platforms detail panel and an automation page", async () => {
    const ctx = createTestExtensionContext();
    await extension.setup(ctx);
    expect(ctx.slots.list().map((item) => item.slot)).toEqual(["platform.detail:bilibili"]);
    expect(ctx.navigation.list().map((item) => [item.activityId, item.pageId])).toEqual([
      ["automation", "bilibili.subscriptions"],
    ]);
    expect(ctx.activities.list()).toEqual([]);
  });
});

describe("account panel", () => {
  test("QR login polls every two seconds until confirmed", async () => {
    vi.useFakeTimers();
    const polls = ["pending", "scanned", "confirmed"];
    const bridge = createFakeBridge({
      handlers: {
        "bilibili.status": () => status(),
        "bilibili.login.start": () => ({ qr_png_base64: "iVBORw0KGgo=" }),
        "bilibili.login.poll": () => ({ status: polls.shift(), message: "" }),
      },
    });
    const { wrapper } = mountPage(BilibiliAccountPanel, { bridge });
    await flushPromises();
    await wrapper.find("button").trigger("click");
    await flushPromises();
    expect(wrapper.find("img").attributes("src")).toBe("data:image/png;base64,iVBORw0KGgo=");
    expect(wrapper.text()).toContain("等待扫码");

    await vi.advanceTimersByTimeAsync(2_000);
    expect(wrapper.text()).toContain("等待扫码");
    await vi.advanceTimersByTimeAsync(2_000);
    expect(wrapper.text()).toContain("已扫码");
    await vi.advanceTimersByTimeAsync(2_000);
    expect(wrapper.find("img").exists()).toBe(false);
    expect(document.body.textContent).toContain("登录成功");
    await vi.advanceTimersByTimeAsync(10_000);
    expect(bridge.calls.filter((call) => call.method === "login.poll")).toHaveLength(3);
    wrapper.unmount();
  });

  test("self-binding is offered only when the owner allows it", async () => {
    const hidden = mountPage(BilibiliAccountPanel, {
      bridge: createFakeBridge({ handlers: { "bilibili.status": () => status() } }),
    });
    await flushPromises();
    expect(hidden.wrapper.text()).not.toContain("自助绑定");
    hidden.wrapper.unmount();

    const verify = vi.fn((params: JsonValue) => {
      expect(params).toEqual({ operator_user_id: "alice", platform: "web", target: { type: "group", group_id: "1001" } });
      return { result: "signature_mismatch", code: "MK-7" };
    });
    const { wrapper } = mountPage(BilibiliAccountPanel, {
      bridge: createFakeBridge({
        handlers: {
          "bilibili.status": () => status({ allow_self_binding: true }),
          "bilibili.binding.start": () => ({ uid: 42, name: "某 UP", code: "MK-7" }),
          "bilibili.binding.verify": verify,
        },
      }),
    });
    await flushPromises();
    const inputs = wrapper.findAll("input");
    await inputs[0].setValue("alice");
    await inputs[1].setValue("42");
    await inputs[2].setValue("1001");
    await button(wrapper.element, "获取验证码").click();
    await flushPromises();
    expect(wrapper.text()).toContain("请把 MK-7 写入 某 UP 的个性签名");
    await button(wrapper.element, "验证绑定").click();
    await flushPromises();
    expect(verify).toHaveBeenCalledOnce();
    expect(wrapper.text()).toContain("个性签名中还没有找到 MK-7");
    wrapper.unmount();
  });

  test("login buttons are disabled for read-only sessions", async () => {
    const { wrapper } = mountPage(BilibiliAccountPanel, {
      bridge: createFakeBridge({ handlers: { "bilibili.status": () => status() } }),
      capabilities: ["runtime.read"],
    });
    await flushPromises();
    expect(button(wrapper.element, "开始扫码登录").disabled).toBe(true);
    wrapper.unmount();
  });
});

describe("subscriptions page", () => {
  test("lists, pauses and deletes behind a danger confirm", async () => {
    const calls: Array<[string, JsonValue]> = [];
    const bridge = createFakeBridge({
      handlers: {
        "bilibili.status": () => status(),
        "bilibili.subscriptions.list": () => ({ subscriptions: [subscription] }),
        "bilibili.subscriptions.set_paused": (params) => {
          calls.push(["pause", params]);
          return { ...subscription, paused: true };
        },
        "bilibili.subscriptions.unsubscribe": (params) => {
          calls.push(["delete", params]);
          return { ok: true };
        },
      },
    });
    const { wrapper } = mountPage(SubscriptionsPage, { bridge });
    await flushPromises();
    expect(wrapper.text()).toContain("群 1001");
    expect(wrapper.text()).toContain("直播、视频");
    expect(wrapper.text()).toContain("推送中");

    await button(wrapper.element, "暂停").click();
    await flushPromises();
    await button(wrapper.element, "删除").click();
    await flushPromises();
    expect(document.querySelector(".mk-dialog")?.textContent).toContain("确认删除订阅 daily（群 1001）");
    dialogButton("删除").click();
    await flushPromises();
    expect(calls).toEqual([
      ["pause", { selector: "daily", paused: true }],
      ["delete", { subscription_id: "daily", confirmed: true }],
    ]);
    wrapper.unmount();
  });

  test("preview links only http(s) urls", async () => {
    const card = { title: "直播开始", description: "desc", url: "javascript:alert(1)" };
    const bridge = createFakeBridge({
      handlers: {
        "bilibili.status": () => status(),
        "bilibili.subscriptions.list": () => ({ subscriptions: [subscription] }),
        "bilibili.subscriptions.preview": () => card,
      },
    });
    const { wrapper } = mountPage(SubscriptionsPage, { bridge });
    await flushPromises();
    await button(wrapper.element, "预览").click();
    await flushPromises();
    const dialog = document.querySelector(".mk-dialog")!;
    expect(dialog.textContent).toContain("直播开始");
    expect(dialog.querySelector("a")).toBeNull();

    dialogButton("关闭")?.click();
    card.url = "https://live.bilibili.com/1";
    await button(wrapper.element, "预览").click();
    await flushPromises();
    expect(document.querySelector(".mk-dialog a")?.getAttribute("href")).toBe("https://live.bilibili.com/1");
    wrapper.unmount();
  });

  test("creates a group subscription with the chosen notifications", async () => {
    const subscribe = vi.fn(() => subscription);
    const bridge = createFakeBridge({
      handlers: {
        "bilibili.status": () => status(),
        "bilibili.subscriptions.list": () => ({ subscriptions: [] }),
        "bilibili.subscriptions.subscribe": subscribe,
      },
    });
    const { wrapper } = mountPage(SubscriptionsPage, { bridge });
    await flushPromises();
    expect(wrapper.text()).toContain("还没有订阅");
    await button(wrapper.element, "新增订阅").click();
    await flushPromises();
    dialogButton("创建订阅").click();
    await flushPromises();
    expect(document.querySelector(".mk-dialog")?.textContent).toContain("请填写订阅名称");
    expect(subscribe).not.toHaveBeenCalled();

    const inputs = document.querySelectorAll<HTMLInputElement>(".mk-dialog input[type=text]");
    for (const [index, value] of ["daily", "42", "qq-main", "1001"].entries()) {
      inputs[index].value = value;
      inputs[index].dispatchEvent(new Event("input"));
    }
    const checks = document.querySelectorAll<HTMLInputElement>(".mk-dialog input[type=checkbox]");
    checks[1].click();
    await flushPromises();
    dialogButton("创建订阅").click();
    await flushPromises();
    expect(subscribe).toHaveBeenCalledWith({
      subscription_id: "daily",
      uid: 42,
      outbound_binding: "qq-main",
      notifications: ["live", "video"],
      target: { type: "group", group_id: "1001" },
    });
    wrapper.unmount();
  });

  test("a disabled owner shows its reason instead of the table", async () => {
    const bridge = createFakeBridge({
      handlers: {
        "bilibili.status": () => status({ management_enabled: false, reason: "未配置 Cookie" }),
        "bilibili.subscriptions.list": () => ({ subscriptions: [] }),
      },
    });
    const { wrapper } = mountPage(SubscriptionsPage, { bridge });
    await flushPromises();
    expect(wrapper.text()).toContain("订阅管理未启用");
    expect(wrapper.text()).toContain("未配置 Cookie");
    expect(button(wrapper.element, "新增订阅").disabled).toBe(true);
    wrapper.unmount();
  });
});
