import { defineExtension } from "@mutsuki/web-sdk";
import { defineComponent, h } from "vue";

import AccountsPage from "./AccountsPage.vue";

/** Generic accounts surface; adapters describe fields, actions and detail panels. */
export default defineExtension({
  id: "mutsuki.bot.platforms",
  setup(ctx) {
    // Detail panels are read when the page renders, so owners loaded later still contribute.
    const Page = defineComponent({
      name: "PlatformAccountsHost",
      setup: () => () => h(AccountsPage, { slots: ctx.slots }),
    });
    ctx.pages.register({
      id: "platforms.accounts",
      path: "/platforms/accounts",
      title: "平台账号",
      component: Page,
      requiredCapability: "bot.platforms.read",
    });
    ctx.navigation.register({
      id: "platforms.accounts.nav",
      activityId: "platforms",
      pageId: "platforms.accounts",
      label: "账号",
      order: 10,
      requiredCapability: "bot.platforms.read",
    });
  },
});
