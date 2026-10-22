import { defineExtension } from "@mutsuki/web-sdk";

import SandboxPage from "./SandboxPage.vue";
import "./sandbox.css";

/** Sandbox conversations (simulate or live) in the `sandbox` activity, on the `sandbox.*` RPCs. */
export default defineExtension({
  id: "mutsuki.bot.sandbox",
  setup(ctx) {
    ctx.pages.register({
      id: "sandbox.page",
      path: "/sandbox",
      title: "沙盒",
      pluginId: "mutsuki.plugin.bot.sandbox",
      component: SandboxPage,
      requiredCapability: "bot.read",
    });
    ctx.navigation.register({
      id: "sandbox.nav",
      activityId: "sandbox",
      pageId: "sandbox.page",
      label: "沙盒",
      order: 10,
      requiredCapability: "bot.read",
    });
  },
});
