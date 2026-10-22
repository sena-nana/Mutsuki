import { createRegistry, RpcError, type SlotRegistration } from "@mutsuki/web-sdk";
import { createFakeBridge, createTestExtensionContext } from "@mutsuki/web-sdk/testing";
import { flushPromises } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { defineComponent, h, onBeforeUnmount } from "vue";

import extension from "../src/index";
import OverviewPage from "../src/OverviewPage.vue";
import { mountPage } from "./mount";

beforeEach(() => {
  const actions = document.createElement("div");
  actions.id = "mk-page-actions";
  document.body.append(actions);
});
afterEach(() => {
  document.body.innerHTML = "";
  vi.useRealTimers();
});

function summary(revision: number) {
  return {
    uptime_ms: 65_000,
    health: { service: revision > 1 ? "degraded" : "ok" },
    counts: { plugins: revision, runners: 2, event_sources: 0 },
  };
}

describe("overview page", () => {
  test("shows the runtime summary and refreshes on control changes", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    let revision = 1;
    const bridge = createFakeBridge({ handlers: { "overview.summary": () => summary(revision) } });
    const { wrapper } = mountPage(OverviewPage, { bridge, props: { slots: createRegistry() } });
    await flushPromises();
    expect(wrapper.text()).toContain("1 分 5 秒");
    expect(wrapper.text()).toContain("正常");

    revision = 2;
    bridge.emit("control.changed", { revision: 2, domains: ["plugins"] });
    await vi.advanceTimersByTimeAsync(60);
    await flushPromises();
    expect(wrapper.text()).toContain("降级");
    expect(bridge.calls.filter((call) => call.method === "summary")).toHaveLength(2);

    document.querySelector<HTMLButtonElement>("#mk-page-actions button")!.click();
    await flushPromises();
    expect(bridge.calls.filter((call) => call.method === "summary")).toHaveLength(3);
    wrapper.unmount();
  });

  test("first-load failure offers a retry", async () => {
    let fail = true;
    const bridge = createFakeBridge({
      handlers: {
        "overview.summary": () => {
          if (fail) throw new RpcError("core_unavailable", "runtime not ready");
          return summary(1);
        },
      },
    });
    const { wrapper } = mountPage(OverviewPage, { bridge, props: { slots: createRegistry() } });
    await flushPromises();
    expect(wrapper.find('[role="alert"]').text()).toContain("runtime not ready");
    fail = false;
    await wrapper.find('[role="alert"] button').trigger("click");
    await flushPromises();
    expect(wrapper.text()).toContain("正常");
    wrapper.unmount();
  });

  test("renders overview card components with their span and unmounts them", async () => {
    const slots = createRegistry<SlotRegistration>();
    const unmounted = vi.fn();
    slots.register({
      id: "qq.card",
      slot: "overview.cards",
      colSpan: 4,
      rowSpan: 2,
      component: defineComponent({
        setup() {
          onBeforeUnmount(unmounted);
          return () => h("p", "QQ 账号");
        },
      }),
    });
    slots.register({ id: "other", slot: "plugin.home", component: defineComponent({ render: () => null }) });
    const bridge = createFakeBridge({ handlers: { "overview.summary": () => summary(1) } });
    const { wrapper } = mountPage(OverviewPage, { bridge, props: { slots } });
    await flushPromises();
    const cards = wrapper.findAll(".overview-cards > article");
    expect(cards).toHaveLength(1);
    expect(cards[0].text()).toBe("QQ 账号");
    expect(cards[0].attributes("style")).toContain("--overview-card-col-span: 4");
    wrapper.unmount();
    expect(unmounted).toHaveBeenCalledOnce();
  });

  test("registers the home page", async () => {
    const ctx = createTestExtensionContext();
    await extension.setup(ctx);
    expect(ctx.navigation.list()[0]).toMatchObject({ activityId: "home", pageId: "overview.page" });
  });
});
