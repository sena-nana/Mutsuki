import { defineExtension } from "@mutsuki/web-sdk";

import BilibiliAccountPanel from "./BilibiliAccountPanel.vue";
import SubscriptionsPage from "./SubscriptionsPage.vue";

const BILIBILI_PLUGIN_ID = "mutsuki.plugin.bot.bilibili";

/**
 * Bilibili's console surface: QR login and self-binding render inside the generic platforms
 * page under the Bilibili account, and push subscriptions live in the automation activity.
 */
export default defineExtension({
  id: "mutsuki.bot.bilibili",
  setup(ctx) {
    ctx.slots.register({
      id: "bilibili.account-panel",
      slot: "platform.detail:bilibili",
      pluginId: BILIBILI_PLUGIN_ID,
      requiredCapability: "runtime.read",
      component: BilibiliAccountPanel,
    });
    ctx.pages.register({
      id: "bilibili.subscriptions",
      path: "/automation/bilibili",
      title: "B 站推送",
      component: SubscriptionsPage,
      pluginId: BILIBILI_PLUGIN_ID,
      requiredCapability: "runtime.read",
    });
    ctx.navigation.register({
      id: "bilibili.subscriptions.nav",
      activityId: "automation",
      pageId: "bilibili.subscriptions",
      label: "B 站推送",
      order: 20,
      requiredCapability: "runtime.read",
    });
  },
});
