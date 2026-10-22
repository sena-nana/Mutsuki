import { RpcError } from "@mutsuki/web-sdk";
import { createFakeBridge, createTestExtensionContext } from "@mutsuki/web-sdk/testing";
import { flushPromises } from "@vue/test-utils";
import { describe, expect, test } from "vitest";

import extension from "../src/index";
import UpgradePage from "../src/UpgradePage.vue";
import { mountPage } from "./mount";

describe("upgrade page", () => {
  test("lists modules, filters on search and shows a plan", async () => {
    const bridge = createFakeBridge({
      handlers: {
        "upgrade.check": (params) => ({
          modules: [{ id: "mutsuki-core", current_revision: "a1", remote_revision: "b2" }].filter((module) =>
            module.id.includes((params as { query: string }).query),
          ),
        }),
        "upgrade.plan": () => ({ module_id: "mutsuki-core", plan: {}, cli_command: "mutsuki-plugin execute mutsuki-core" }),
      },
    });
    const { wrapper } = mountPage(UpgradePage, { bridge });
    await flushPromises();
    expect(wrapper.text()).toContain("mutsuki-core");
    expect(wrapper.text()).toContain("a1 → b2");

    await wrapper.findAll("button").find((button) => button.text() === "生成升级计划")!.trigger("click");
    await flushPromises();
    expect(wrapper.find("pre").text()).toBe("mutsuki-plugin execute mutsuki-core");
    expect(bridge.calls.filter((call) => call.method === "plan")[0].operation).toBe("read");

    await wrapper.find('input[type="search"]').setValue("nothing");
    await wrapper.find('input[type="search"]').trigger("keydown", { key: "Enter" });
    await wrapper.findAll("button").find((button) => button.text() === "检查更新")!.trigger("click");
    await flushPromises();
    expect(bridge.calls.at(-1)?.params).toEqual({ query: "nothing" });
    expect(wrapper.text()).toContain("没有可更新的模块");
    wrapper.unmount();
  });

  test("a failed check shows a retryable error", async () => {
    let fail = true;
    const bridge = createFakeBridge({
      handlers: {
        "upgrade.check": () => {
          if (fail) throw new RpcError("rpc_failed", "release set unreadable");
          return { modules: [] };
        },
      },
    });
    const { wrapper } = mountPage(UpgradePage, { bridge });
    await flushPromises();
    expect(wrapper.find('[role="alert"]').text()).toContain("release set unreadable");
    fail = false;
    await wrapper.find('[role="alert"] button').trigger("click");
    await flushPromises();
    expect(wrapper.text()).toContain("没有可更新的模块");
    wrapper.unmount();
  });

  test("registers its page under the system activity", async () => {
    const ctx = createTestExtensionContext();
    await extension.setup(ctx);
    expect(ctx.pages.list().map((page) => page.id)).toEqual(["upgrade.page"]);
    expect(ctx.navigation.list()[0]).toMatchObject({ activityId: "system", pageId: "upgrade.page" });
  });
});
