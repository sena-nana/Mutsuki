import { defineExtension } from "@mutsuki/web-sdk";

import DeliveriesPage from "./DeliveriesPage.vue";
import InteractionsPage from "./InteractionsPage.vue";
import QqAccountsCard from "./QqAccountsCard.vue";

const QQ_PROVIDER_ID = "mutsuki.plugin.bot.adapter-qqbot";

/**
 * QQ's console surface: the overview card, plus delivery and interaction pages that stay on the
 * QQ owner's RPCs until a platform-neutral delivery namespace exists. Accounts are managed on
 * the generic platforms page.
 */
export default defineExtension({
  id: "mutsuki.bot.qq",
  setup(ctx) {
    ctx.slots.register({
      id: "qq-bot.overview.cards",
      slot: "overview.cards",
      pluginId: QQ_PROVIDER_ID,
      requiredCapability: "bot.read",
      component: QqAccountsCard,
    });
    const pages = [
      ["qq-bot.deliveries", "/platforms/deliveries", "投递记录", 20, DeliveriesPage],
      ["qq-bot.interactions", "/platforms/interactions", "交互等待", 30, InteractionsPage],
    ] as const;
    for (const [id, path, title, order, component] of pages) {
      ctx.pages.register({ id, path, title, component, requiredCapability: "bot.read", pluginId: QQ_PROVIDER_ID });
      ctx.navigation.register({
        id: `${id}.nav`,
        activityId: "platforms",
        pageId: id,
        label: title,
        order,
        requiredCapability: "bot.read",
      });
    }
  },
});
