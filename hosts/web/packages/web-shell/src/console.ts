import { installMutsukiUi, uiMessages } from "@mutsuki/ui";
import {
  QueryBus,
  createCookieSession,
  createI18n,
  provideI18n,
  provideMutsukiRuntime,
  type Disposable,
} from "@mutsuki/web-sdk";
import { createApp, defineComponent, h, provide, ref, shallowRef, type PropType } from "vue";
import { createRouter, createWebHashHistory, type Router } from "vue-router";

import App from "./app/App.vue";
import ConsoleRoot from "./app/ConsoleRoot.vue";
import SettingsPage from "./app/SettingsPage.vue";
import { SHELL_CONTEXT, type ShellContext } from "./app/context.js";
import type { ConsoleLayout, ShellFailure } from "./layout.js";
import { shellMessages } from "./messages.js";
import { buildShellModel, type ShellModel } from "./navigation.js";
import {
  createWebShellRuntime,
  createWebUiThemeController,
  type ExtensionModuleRef,
  type WebShellRuntime,
} from "./runtime.js";

export interface ConsoleOptions {
  /** Product information architecture; the only source of activities. */
  layout: ConsoleLayout;
  extensions: ExtensionModuleRef[];
  brand?: string;
  /** Page shown for `#/` and unknown routes. */
  homePageId?: string;
  /** Defaults to `/ws` on the document's host. */
  bridgeUrl?: string;
  /** Activity that hosts the shell's own settings page (default `settings`). */
  settingsActivityId?: string;
}

export const SETTINGS_PAGE_ID = "webui.settings";

function defaultBridgeUrl(): string {
  const protocol = location.protocol === "https:" ? "wss" : "ws";
  return `${protocol}://${location.host}/ws`;
}

const ShellProvider = defineComponent({
  name: "ShellProvider",
  props: { context: { type: Object as PropType<ShellContext>, required: true } },
  setup(props) {
    provide(SHELL_CONTEXT, props.context);
    return () => h(App);
  },
});

const queries = new QueryBus();

/**
 * Mounts the console: cookie session (or login), extension loading, product layout, hash
 * routing and per-extension error isolation.
 */
export function startConsole(root: HTMLElement, options: ConsoleOptions): Disposable {
  const brand = options.brand ?? "Mutsuki";
  const theme = createWebUiThemeController();
  const failures = ref<ShellFailure[]>([]);
  const context = shallowRef<ShellContext | null>(null);
  let runtime: WebShellRuntime | null = null;
  const session = createCookieSession(() => new Set(runtime?.state.capabilities ?? []));
  const i18n = createI18n().extend("ui", uiMessages).extend("shell", shellMessages);

  const router: Router = createRouter({
    history: createWebHashHistory(),
    routes: [
      { path: "/:activity/:page", component: { render: () => null } },
      { path: "/:pathMatch(.*)*", component: { render: () => null } },
    ],
  });
  /** `path` when it names an enabled page, else the home route (null without any page). */
  function landing(model: ShellModel, path: string): string | null {
    const { params } = router.resolve(path);
    const activityId = params.activity;
    const pageId = params.page;
    const valid =
      typeof activityId === "string" &&
      typeof pageId === "string" &&
      model.navByActivity.get(activityId)?.some((item) => item.pageId === pageId && !item.disabled);
    if (valid) return path;
    const first = model.firstRoute();
    return first ? model.routeFor(first.activityId, first.pageId) : null;
  }

  router.beforeEach((to) => {
    const model = context.value?.model;
    if (!model) return true;
    const target = landing(model, to.fullPath);
    return target === null || target === to.fullPath ? true : target;
  });

  async function start(): Promise<WebShellRuntime> {
    runtime?.dispose();
    const next = createWebShellRuntime({ bridgeUrl: options.bridgeUrl ?? defaultBridgeUrl() });
    runtime = next;
    try {
      await next.connect();
    } catch (error) {
      next.dispose();
      runtime = null;
      throw error;
    }
    next.configureLayout(options.layout);
    next.state.pages.register({
      id: SETTINGS_PAGE_ID,
      path: "/settings",
      title: "设置",
      component: SettingsPage,
    });
    next.state.navigation.register({
      id: `${SETTINGS_PAGE_ID}.nav`,
      activityId: options.settingsActivityId ?? "settings",
      pageId: SETTINGS_PAGE_ID,
      label: "设置",
      order: 1000,
    });
    await next.load(options.extensions);
    const can = (capability?: string) =>
      !capability || next.state.capabilities.has("*") || next.state.capabilities.has(capability);
    const model = buildShellModel(next.state, options.layout, can, options.homePageId);
    failures.value = [...next.state.failures, ...model.failures];
    context.value = {
      runtime: next,
      layout: options.layout,
      model,
      brand,
      theme,
      failures,
      reportFailure(failure) {
        failures.value = [...failures.value, failure];
      },
      async logout() {
        await session.logout();
        window.location.reload();
      },
    };
    // The router settled on `#/` (or a stale route) before any page existed; land explicitly,
    // since replacing the current location with itself would not re-run the guard.
    const legacyPage = new URLSearchParams(location.search).get("page");
    const requested =
      (legacyPage ? placedRouteForPath(context.value, `/${legacyPage}`) : null) ??
      router.currentRoute.value.fullPath;
    const target = landing(model, requested);
    if (target && target !== router.currentRoute.value.fullPath) await router.replace(target);
    return next;
  }

  const app = createApp({
    render: () =>
      h(
        ConsoleRoot,
        { brand, start, login: (token: string) => session.login(token) },
        {
          default: () =>
            context.value
              ? h(ShellProvider, { context: context.value, key: context.value.runtime.session?.session_id })
              : null,
        },
      ),
  });
  provideI18n(app, i18n);
  installMutsukiUi(app);
  provideMutsukiRuntime(app, {
    get client() {
      if (!runtime) throw new Error("console runtime is not connected");
      return runtime.bridge;
    },
    session,
    queries,
  });
  app.use(router);
  app.mount(root);

  const onPageHide = () => dispose();
  window.addEventListener("pagehide", onPageHide, { once: true });
  function dispose() {
    window.removeEventListener("pagehide", onPageHide);
    app.unmount();
    runtime?.dispose();
    theme.dispose();
  }
  return { dispose };
}

function placedRouteForPath(context: ShellContext, path: string): string | null {
  for (const [activityId, items] of context.model.navByActivity) {
    for (const item of items) {
      if (context.model.pages.get(item.pageId)?.path === path && !item.disabled) {
        return context.model.routeFor(activityId, item.pageId);
      }
    }
  }
  return null;
}
