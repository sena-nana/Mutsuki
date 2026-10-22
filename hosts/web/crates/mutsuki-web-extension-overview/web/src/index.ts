import { defineExtension } from "@mutsuki/web-sdk";
import { defineComponent, h } from "vue";

import OverviewPage from "./OverviewPage.vue";

export default defineExtension({
  id: "mutsuki.web.overview",
  setup(ctx) {
    // Cards are read when the page renders, so extensions loaded later still appear.
    const Page = defineComponent({
      name: "OverviewPageHost",
      setup: () => () => h(OverviewPage, { slots: ctx.slots }),
    });
    ctx.navigation.register({
      id: "overview.nav",
      activityId: "home",
      pageId: "overview.page",
      label: "概览",
      order: 1,
      requiredCapability: "runtime.read",
    });
    ctx.pages.register({
      id: "overview.page",
      path: "/overview",
      title: "概览",
      component: Page,
      requiredCapability: "runtime.read",
    });
  },
});
