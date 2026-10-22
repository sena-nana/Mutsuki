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
    // The plugin list (deployments, reload) heads the `plugins` activity above the plugin hubs.
    const pages: Array<[id: string, activity: string, path: string, title: string, order: number, component: Component]> = [
      ["control.plugins", "plugins", "/plugins", "插件列表", -10, Plugins],
      ["control.runners", "system", "/runners", "运行器", 20, RunnersPage],
      ["control.events", "system", "/events", "事件源", 30, EventSourcesPage],
      ["control.topology", "system", "/topology", "拓扑", 40, TopologyPage],
      ["control.tasks", "system", "/tasks", "任务", 50, TasksPage],
      ["control.lifecycle", "system", "/lifecycle", "生命周期", 70, LifecyclePage],
      ["control.logs", "system", "/logs", "日志", 80, LogsPage],
    ];
    for (const [id, activityId, path, title, order, component] of pages) {
      ctx.pages.register({ id, path, title, component, requiredCapability: "runtime.read" });
      ctx.navigation.register({
        id: `${id}.nav`,
        activityId,
        pageId: id,
        label: title,
        order,
        requiredCapability: "runtime.read",
      });
    }
  },
});
