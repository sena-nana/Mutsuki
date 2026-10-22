import { defineExtension } from "@mutsuki/web-sdk";

import UpgradePage from "./UpgradePage.vue";

export default defineExtension({
  id: "mutsuki.web.upgrade",
  setup(ctx) {
    ctx.pages.register({
      id: "upgrade.page",
      path: "/upgrade",
      title: "升级",
      component: UpgradePage,
      requiredCapability: "runtime.read",
    });
    ctx.navigation.register({
      id: "upgrade.nav",
      activityId: "system",
      pageId: "upgrade.page",
      label: "升级",
      order: 90,
      requiredCapability: "runtime.read",
    });
  },
});
