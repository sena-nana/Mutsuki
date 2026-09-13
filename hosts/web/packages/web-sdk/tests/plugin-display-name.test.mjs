import assert from "node:assert/strict";
import test from "node:test";

import { pluginDisplayName } from "../dist/index.js";

function state(pages, navigation = []) {
  return {
    pages: { list: () => pages },
    navigation: { list: () => navigation },
  };
}

test("pluginDisplayName prefers a named hub, then extra pages, then the plugin id", () => {
  assert.equal(
    pluginDisplayName(
      state(
        [
          { id: "mutsuki.bot.router.flow", title: "流程编辑器", pluginId: "mutsuki.bot.router.flow" },
          { id: "bot-flow.page", title: "流程编排", pluginId: "mutsuki.bot.router.flow" },
        ],
        [
          { pageId: "mutsuki.bot.router.flow", label: "流程编辑器" },
          { pageId: "bot-flow.page", label: "流程编排" },
        ],
      ),
      "mutsuki.bot.router.flow",
    ),
    "流程编辑器",
  );
  assert.equal(
    pluginDisplayName(
      state(
        [
          { id: "mutsuki.plugin.bot.agent", title: "回复", pluginId: "mutsuki.plugin.bot.agent" },
          {
            id: "bot-agent.page",
            title: "会话",
            pluginIds: ["mutsuki.agent.runtime.local", "mutsuki.plugin.bot.agent"],
          },
        ],
        [
          { pageId: "mutsuki.plugin.bot.agent", label: "回复" },
          { pageId: "bot-agent.page", label: "会话" },
        ],
      ),
      "mutsuki.plugin.bot.agent",
    ),
    "回复",
  );
  assert.equal(
    pluginDisplayName(
      state(
        [
          { id: "mutsuki.bot.sandbox", title: "mutsuki.bot.sandbox", pluginId: "mutsuki.bot.sandbox" },
          { id: "sandbox.page", title: "沙盒", pluginId: "mutsuki.bot.sandbox" },
        ],
        [
          { pageId: "mutsuki.bot.sandbox", label: "mutsuki.bot.sandbox" },
          { pageId: "sandbox.page", label: "沙盒" },
        ],
      ),
      "mutsuki.bot.sandbox",
    ),
    "沙盒",
  );
  assert.equal(
    pluginDisplayName(
      state(
        [{ id: "extra", title: "沙盒", pluginId: "mutsuki.bot.sandbox" }],
        [{ pageId: "extra", label: "mutsuki.bot.sandbox" }],
      ),
      "mutsuki.bot.sandbox",
    ),
    "沙盒",
  );
  assert.equal(pluginDisplayName(state([]), "mutsuki.bot.command"), "mutsuki.bot.command");
  assert.equal(pluginDisplayName(null, "mutsuki.bot.command"), "mutsuki.bot.command");
});
