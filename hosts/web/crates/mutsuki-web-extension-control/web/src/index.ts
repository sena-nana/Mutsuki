import { defineExtension, type ExtensionContext } from "@mutsuki/web-sdk";
import { defineComponent, h, type Component } from "vue";

import EventSourcesPage from "./pages/EventSourcesPage.vue";
import LifecyclePage from "./pages/LifecyclePage.vue";
import LogsPage from "./pages/LogsPage.vue";
import PluginsPage from "./pages/PluginsPage.vue";
import RunnersPage from "./pages/RunnersPage.vue";
import TasksPage from "./pages/TasksPage.vue";
import TopologyPage from "./pages/TopologyPage.vue";

export { projectRuntimeEvents } from "./runtime-events";

/** Diagnostics over the Control API; page ids stay `control.*` so saved routes keep working. */
export default defineExtension({
  id: "mutsuki.web.diagnostics",
  setup(ctx) {
    const registries: Pick<ExtensionContext, "pages" | "navigation"> = {
      pages: ctx.pages,
      navigation: ctx.navigation,
    };
    const Plugins = defineComponent({
      name: "DiagnosticsPlugins",
      setup: () => () => h(PluginsPage, { registries }),
    });
    const pages: Array<[id: string, path: string, title: string, order: number, component: Component]> = [
      ["control.plugins", "/plugins", "插件", 10, Plugins],
      ["control.runners", "/runners", "运行器", 20, RunnersPage],
      ["control.events", "/events", "事件源", 30, EventSourcesPage],
      ["control.topology", "/topology", "拓扑", 40, TopologyPage],
      ["control.tasks", "/tasks", "任务", 50, TasksPage],
      ["control.lifecycle", "/lifecycle", "生命周期", 70, LifecyclePage],
      ["control.logs", "/logs", "日志", 80, LogsPage],
    ];
    for (const [id, path, title, order, component] of pages) {
      ctx.pages.register({ id, path, title, component, requiredCapability: "runtime.read" });
      ctx.navigation.register({
        id: `${id}.nav`,
        activityId: "system",
        pageId: id,
        label: title,
        order,
        requiredCapability: "runtime.read",
      });
    }
  },
});
