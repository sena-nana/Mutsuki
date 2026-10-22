import { RpcError, type JsonValue } from "@mutsuki/web-sdk";
import { createFakeBridge, createTestExtensionContext } from "@mutsuki/web-sdk/testing";
import { flushPromises } from "@vue/test-utils";
import { afterEach, describe, expect, test, vi } from "vitest";

import { composePayload, imageUrl, keyboardSegment, keyboardSource, messageParts, resolveIdentity } from "../src/content";
import type { Message, Snapshot } from "../src/contract";
import extension from "../src/index";
import SandboxPage from "../src/SandboxPage.vue";
import { mountPage } from "./mount";

afterEach(() => {
  document.body.innerHTML = "";
});

const message = (patch: Partial<Message>): Message => ({
  message_id: "m1",
  conversation_id: "c1",
  sender_id: "u1",
  sender_name: "阿光",
  role: "user",
  text: "",
  time_ms: 1_700_000_000_000,
  ...patch,
});

const snapshot = (patch: Partial<Snapshot> = {}): Snapshot => ({
  revision: 4,
  mode: "simulate",
  live_available: true,
  conversations: [
    {
      conversation_id: "c1",
      kind: "group",
      title: "测试群",
      users: [
        { user_id: "u1", display_name: "阿光" },
        { user_id: "u2", display_name: "小雪" },
      ],
      last_preview: "你好",
      last_activity_unix_ms: Date.now(),
      active_message: true,
    },
  ],
  live_users: [{ user_id: "u9", display_name: "真实用户" }],
  bot: { user_id: "bot", display_name: "睦月" },
  ...patch,
});

const history: Message[] = [
  message({ message_id: "m1", text: "早上好 <b>x</b>" }),
  message({ message_id: "m2", role: "bot", sender_id: "bot", sender_name: "睦月", text: "早", reply_to: "m1" }),
];

function sandboxBridge(handlers: Record<string, (params: JsonValue) => unknown> = {}, state = snapshot()) {
  return createFakeBridge({
    handlers: {
      "sandbox.snapshot": () => state,
      "sandbox.messages": () => history,
      "sandbox.write": () => ({ revision: 5, result: {} }),
      ...handlers,
    },
  });
}

const button = (label: string) =>
  [...document.querySelectorAll<HTMLButtonElement>("button")].find((item) => item.textContent?.trim() === label || item.getAttribute("aria-label") === label)!;

describe("content", () => {
  test("mentions, images and markup render as parts, never as HTML", () => {
    const parts = messageParts(
      message({
        text: "@小雪 看[图片]",
        refs: [
          { t: "mention", at: 0, id: "u2", name: "小雪" },
          { t: "img", at: 5, h: "media-1", mime: "image/png", name: "a.png" },
        ],
      }),
    );
    expect(parts).toEqual([
      { kind: "mention", text: "@小雪" },
      { kind: "text", text: " 看" },
      { kind: "media", mime: "image/png", url: "", mediaId: "media-1", name: "a.png" },
    ]);
  });

  test("only https images load; QQ http CDN links are upgraded", () => {
    expect(imageUrl("http://q.qlogo.cn/a.png")).toBe("https://q.qlogo.cn/a.png");
    expect(imageUrl("//q.qlogo.cn/a.png")).toBe("https://q.qlogo.cn/a.png");
    expect(imageUrl("javascript:alert(1)")).toBe("");
  });

  test("markdown carries only buttons; buttons need markdown", () => {
    const keyboard = keyboardSegment("确认 | /ok\n取消")!;
    expect(keyboardSource(keyboard)).toBe("确认 | /ok\n取消");
    expect(composePayload("**hi**", [keyboard], true)).toEqual({ ok: true, text: "", segments: [{ type: "markdown", content: "**hi**" }, keyboard] });
    expect(composePayload("hi", [keyboard], false)).toEqual({ ok: false, reason: "keyboardNeedsMarkdown" });
    expect(composePayload("hi", [{ type: "mention_all" }], true)).toEqual({ ok: false, reason: "markdownMixed" });
    expect(composePayload("  ", [], false)).toEqual({ ok: false, reason: "empty" });
  });

  test("the simulated speaker is never the bot", () => {
    const state = snapshot();
    expect(resolveIdentity(state.conversations[0], state, "bot")).toBe("u1");
    expect(resolveIdentity(state.conversations[0], state, "u2")).toBe("u2");
  });
});

describe("page", () => {
  test("renders conversations and messages as text and speaks as the chosen member", async () => {
    const writes: JsonValue[] = [];
    const bridge = sandboxBridge({
      "sandbox.write": (params) => {
        writes.push(params);
        return { revision: 5, result: {} };
      },
    });
    const { wrapper } = mountPage(SandboxPage, { bridge });
    await flushPromises();
    expect(wrapper.text()).toContain("测试群");
    expect(wrapper.find(".sandbox-messages").text()).toContain("早上好 <b>x</b>");
    expect(wrapper.find(".sandbox-messages b").exists()).toBe(false);
    expect(wrapper.find(".sandbox-quote").text()).toBe("阿光: 早上好 <b>x</b>");

    // Pick 小雪 as the speaker, quote 阿光 and send.
    await wrapper.findAll(".sandbox-member-pick").find((item) => item.text() === "小雪")!.trigger("click");
    await wrapper.find(".sandbox-reply").trigger("click");
    expect(wrapper.text()).toContain("正在回复 阿光");
    const input = wrapper.find(".sandbox-compose input");
    expect(input.attributes("placeholder")).toBe("以 小雪 的身份发给机器人，Enter 发送");
    await input.setValue("你好");
    await input.trigger("keydown", { key: "Enter" });
    await flushPromises();
    expect(writes).toHaveLength(1);
    expect(writes[0]).toMatchObject({
      confirmed: false,
      request: {
        expected_revision: 4,
        action: { action: "ingest_as_user", conversation_id: "c1", user_id: "u2", text: "你好", segments: [], reply_to: "m1" },
      },
    });
    expect((wrapper.find(".sandbox-compose input").element as HTMLInputElement).value).toBe("");
    wrapper.unmount();
  });

  test("typing @ offers members and adds a mention segment", async () => {
    const write = vi.fn((_params: JsonValue) => ({ revision: 5, result: {} }));
    const { wrapper } = mountPage(SandboxPage, { bridge: sandboxBridge({ "sandbox.write": write }) });
    await flushPromises();
    await wrapper.find(".sandbox-compose input").setValue("看这里 @小");
    const picker = wrapper.find(".sandbox-mention-picker");
    expect(picker.findAll("button").map((item) => item.text())).toEqual(["@小雪"]);
    await picker.find("button").trigger("click");
    expect(wrapper.find(".sandbox-draft-chip").text()).toContain("@小雪");
    await wrapper.find(".sandbox-compose input").trigger("keydown", { key: "Enter" });
    await flushPromises();
    expect((write.mock.calls[0][0] as { request: { action: { text: string; segments: unknown[] } } }).request.action).toMatchObject({
      text: "看这里",
      segments: [{ type: "mention_user", user_id: "u2" }],
    });
    wrapper.unmount();
  });

  test("live sends ask first and go out as the bot with confirmation", async () => {
    const write = vi.fn((_params: JsonValue) => ({ revision: 5, result: {} }));
    const { wrapper } = mountPage(SandboxPage, { bridge: sandboxBridge({ "sandbox.write": write }, snapshot({ mode: "live" })) });
    await flushPromises();
    await wrapper.find(".sandbox-compose input").setValue("公告");
    button("发送到 QQ").click();
    await flushPromises();
    expect(write).not.toHaveBeenCalled();
    [...document.querySelectorAll<HTMLButtonElement>(".mk-dialog button")].find((item) => item.textContent?.trim() === "发送到 QQ")!.click();
    await flushPromises();
    expect(write.mock.calls[0][0]).toMatchObject({ confirmed: true, request: { action: { action: "send_as_bot", text: "公告" } } });
    wrapper.unmount();
  });

  test("clearing a conversation needs a danger confirm; failures surface as toasts", async () => {
    const bridge = sandboxBridge({
      "sandbox.write": () => {
        throw new RpcError("sandbox.revision_conflict", "沙盒已在别处更新");
      },
    });
    const { wrapper } = mountPage(SandboxPage, { bridge });
    await flushPromises();
    button("清空").click();
    await flushPromises();
    expect(document.querySelector(".mk-dialog")?.textContent).toContain("清空「测试群」的模拟消息？");
    [...document.querySelectorAll<HTMLButtonElement>(".mk-dialog button")].find((item) => item.textContent?.trim() === "清空")!.click();
    await flushPromises();
    expect(document.body.textContent).toContain("操作失败：沙盒已在别处更新");
    wrapper.unmount();
  });

  test("importing live members writes their ids", async () => {
    const write = vi.fn((_params: JsonValue) => ({ revision: 5, result: {} }));
    const { wrapper } = mountPage(SandboxPage, { bridge: sandboxBridge({ "sandbox.write": write }) });
    await flushPromises();
    button("导入").click();
    await flushPromises();
    button("全部导入").click();
    await flushPromises();
    expect(write.mock.calls[0][0]).toMatchObject({ request: { action: { action: "import_live_users", user_ids: ["u9"] } } });
    wrapper.unmount();
  });
});

test("registers the sandbox page without an activity", async () => {
  const ctx = createTestExtensionContext();
  await extension.setup(ctx);
  expect(extension.id).toBe("mutsuki.bot.sandbox");
  expect(ctx.navigation.list().map((item) => [item.activityId, item.pageId])).toEqual([["sandbox", "sandbox.page"]]);
});
