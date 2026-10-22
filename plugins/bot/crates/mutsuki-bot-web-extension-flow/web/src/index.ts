import { defineExtension } from "@mutsuki/web-sdk";

import FlowEditorPage from "./FlowEditorPage.vue";
import "./node-editor.css";

/** Bot Flow node editor in the `automation` activity. */
export default defineExtension({
  id: "mutsuki.bot.flow",
  setup(ctx) {
    ctx.pages.register({
      id: "bot-flow.page",
      path: "/flows",
      title: "流程编排",
      pluginId: "mutsuki.plugin.bot.flow",
      component: FlowEditorPage,
      requiredCapability: "bot.flow.read",
    });
    ctx.navigation.register({
      id: "bot-flow.nav",
      activityId: "automation",
      pageId: "bot-flow.page",
      label: "流程编排",
      order: 10,
      requiredCapability: "bot.flow.read",
    });
  },
});
