import { defineExtension } from "@mutsuki/web-sdk";

import DatabasePage from "./DatabasePage.vue";

export default defineExtension({
  id: "mutsuki.bot.database",
  setup(ctx) {
    ctx.navigation.register({
      id: "database.nav",
      activityId: "data",
      pageId: "database.page",
      label: "数据库",
      order: 20,
      requiredCapability: "runtime.read",
    });
    ctx.pages.register({
      id: "database.page",
      path: "/database",
      title: "数据库",
      component: DatabasePage,
      requiredCapability: "runtime.read",
    });
  },
});
