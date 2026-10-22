import { RpcError, type JsonValue } from "@mutsuki/web-sdk";
import { createFakeBridge, createTestExtensionContext } from "@mutsuki/web-sdk/testing";
import { flushPromises } from "@vue/test-utils";
import { afterEach, describe, expect, test } from "vitest";

import DeliveriesPage from "../src/DeliveriesPage.vue";
import extension from "../src/index";
import InteractionsPage from "../src/InteractionsPage.vue";
import QqAccountsCard from "../src/QqAccountsCard.vue";
import { mountPage } from "./mount";

afterEach(() => {
  document.body.innerHTML = "";
});

const dialogButton = (label: string) =>
  [...document.querySelectorAll<HTMLButtonElement>(".mk-dialog button")].find((item) => item.textContent?.trim() === label)!;

const snapshot = {
  revision: 7,
  accounts: [
    {
      account_id: "main",
      enabled: true,
      health: "ok",
      connection_state: "identified",
      connected_since_unix_ms: Date.now() - 120_000,
      self_user: { display_name: "睦月", avatar_url: "http://q.qlogo.cn/a.png" },
    },
    { account_id: "spare", enabled: true, health: "unhealthy", connection_state: "disconnected", last_error: "token expired", self_user: { avatar_url: "https://evil.example/x.png" } },
  ],
};

describe("overview card", () => {
  test("lists accounts with https QQ avatars only", async () => {
    const bridge = createFakeBridge({ handlers: { "qq-bot.snapshot": () => snapshot } });
    const { wrapper } = mountPage(QqAccountsCard, { bridge });
    await flushPromises();
    const images = wrapper.findAll("img");
    expect(images).toHaveLength(1);
    expect(images[0].attributes("src")).toBe("https://q.qlogo.cn/a.png");
    expect(images[0].attributes("alt")).toBe("睦月 的头像");
    expect(wrapper.text()).toContain("正常 · 已上线");
    expect(wrapper.text()).toContain("异常 · 未连接");
    expect(wrapper.text()).toContain("token expired");
    wrapper.unmount();
  });

  test("an unconfigured owner is a hint, not an error", async () => {
    const bridge = createFakeBridge({
      handlers: {
        "qq-bot.snapshot": () => {
          throw new RpcError("qq.owner_unavailable", "QQ 尚未连接");
        },
      },
    });
    const { wrapper } = mountPage(QqAccountsCard, { bridge });
    await flushPromises();
    expect(wrapper.text()).toContain("尚未登录 QQ");
    expect(wrapper.find('[role="alert"]').exists()).toBe(false);
    wrapper.unmount();
  });
});

describe("deliveries", () => {
  const page = {
    items: [
      { receipt: { delivery_id: "d1", status: "permanently_failed", attempt_count: 3, error_code: "qq.rate_limited" }, attempts: [] },
      { receipt: { delivery_id: "d2", status: "succeeded", attempt_count: 1, delivered_at_unix_ms: 1 }, attempts: [] },
    ],
    next_cursor: null,
  };

  test("retry is confirmed and written against the current revision", async () => {
    const writes: JsonValue[] = [];
    const bridge = createFakeBridge({
      handlers: {
        "qq-bot.deliveries.list": () => page,
        "qq-bot.snapshot": () => snapshot,
        "qq-bot.write": (params) => {
          writes.push(params);
          return { revision: 8, audit_id: "a", result: { status: "previewed", text: "hi" } };
        },
      },
    });
    const { wrapper } = mountPage(DeliveriesPage, { bridge });
    await flushPromises();
    const rows = wrapper.findAll("tbody tr");
    expect(rows[0].text()).toContain("失败");
    expect(rows[0].text()).toContain("qq.rate_limited");
    expect(rows[1].findAll("button").map((button) => button.text())).toEqual(["预览"]);

    await rows[0].findAll("button").find((button) => button.text() === "重试")!.trigger("click");
    await flushPromises();
    dialogButton("确认").click();
    await flushPromises();
    expect(writes[0]).toEqual({
      request: { operation_id: "", expected_revision: 7, action: { action: "delivery_retry", delivery_id: "d1" } },
      confirmed: true,
    });

    await rows[1].find("button").trigger("click");
    await flushPromises();
    expect(document.querySelector(".mk-dialog pre")?.textContent).toContain('"status": "previewed"');
    wrapper.unmount();
  });

  test("a failed write surfaces as a toast", async () => {
    const bridge = createFakeBridge({
      handlers: {
        "qq-bot.deliveries.list": () => page,
        "qq-bot.snapshot": () => snapshot,
        "qq-bot.write": () => {
          throw new RpcError("qq.revision_conflict", "revision changed");
        },
      },
    });
    const { wrapper } = mountPage(DeliveriesPage, { bridge });
    await flushPromises();
    await wrapper.findAll("tbody tr")[0].findAll("button").find((button) => button.text() === "重试")!.trigger("click");
    await flushPromises();
    dialogButton("确认").click();
    await flushPromises();
    expect(document.querySelector(".mk-toast--err")?.textContent).toContain("revision changed");
    wrapper.unmount();
  });
});

describe("interactions", () => {
  test("only waiting sessions can be cancelled, after confirmation", async () => {
    const writes: JsonValue[] = [];
    const bridge = createFakeBridge({
      handlers: {
        "qq-bot.interactions.list": () => ({
          items: [
            { session_id: "s1", conversation: { platform: "qqbot", account_id: "main", kind: "group", group_id: "g1" }, status: "waiting", retries_remaining: 2, wait: { timeout_at_unix_ms: Date.now() + 60_000 } },
            { session_id: "s2", conversation: { platform: "qqbot", account_id: "main", kind: "private", user_id: "u1" }, status: "completed", retries_remaining: 0, wait: { timeout_at_unix_ms: 1 } },
          ],
          next_cursor: null,
        }),
        "qq-bot.snapshot": () => snapshot,
        "qq-bot.write": (params) => {
          writes.push(params);
          return { revision: 8, audit_id: "a", result: null };
        },
      },
    });
    const { wrapper } = mountPage(InteractionsPage, { bridge });
    await flushPromises();
    const rows = wrapper.findAll("tbody tr");
    expect(rows[0].text()).toContain("group · g1");
    expect(rows[1].find("button").exists()).toBe(false);
    await rows[0].find("button").trigger("click");
    await flushPromises();
    dialogButton("确认").click();
    await flushPromises();
    expect((writes[0] as { request: { action: unknown } }).request.action).toEqual({ action: "interaction_cancel", session_id: "s1" });
    wrapper.unmount();
  });
});

describe("extension", () => {
  test("contributes the overview card and two platforms pages, no account management", async () => {
    const ctx = createTestExtensionContext();
    await extension.setup(ctx);
    expect(ctx.slots.list().map((item) => item.slot)).toEqual(["overview.cards"]);
    expect(ctx.navigation.list().map((item) => [item.activityId, item.pageId])).toEqual([
      ["platforms", "qq-bot.deliveries"],
      ["platforms", "qq-bot.interactions"],
    ]);
  });
});
