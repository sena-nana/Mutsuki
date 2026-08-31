// src/runtime.ts
import {
  WebBridgeClient,
  DisposableScope,
  createRegistry,
  pluginIdsOf
} from "./web-sdk.js";

// ../ui/dist/theme.js
function applyTheme(theme = "dark") {
  const root = document.documentElement;
  if (theme === "light") {
    root.dataset.theme = "light";
  } else {
    delete root.dataset.theme;
  }
  root.style.colorScheme = theme;
}

// src/runtime.ts
var WEB_UI_THEME_STORAGE_KEY = "mutsuki.webui.theme";
function createWebUiThemeController(options = {}) {
  const storage = options.storage ?? globalThis.localStorage;
  const media = options.media ?? globalThis.matchMedia?.("(prefers-color-scheme: light)");
  const storageKey = options.storageKey ?? WEB_UI_THEME_STORAGE_KEY;
  const stored = storage?.getItem(storageKey);
  let preference = isThemePreference(stored) ? stored : "system";
  let disposed = false;
  const apply = () => applyTheme(
    preference === "system" ? media?.matches ? "light" : "dark" : preference
  );
  const mediaListener = () => {
    if (!disposed && preference === "system") apply();
  };
  media?.addEventListener("change", mediaListener);
  apply();
  return {
    get preference() {
      return preference;
    },
    setPreference(next) {
      if (disposed || next === preference) return;
      preference = next;
      storage?.setItem(storageKey, next);
      apply();
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      media?.removeEventListener("change", mediaListener);
    }
  };
}
function isThemePreference(value) {
  return value === "system" || value === "light" || value === "dark";
}
function groupNavigationItems(items) {
  const sections = [];
  for (const item of items) {
    const last = sections.at(-1);
    if (!last || last.group !== item.group) {
      sections.push({ group: item.group, items: [item] });
    } else {
      last.items.push(item);
    }
  }
  return sections;
}
function createShellState() {
  return {
    extensions: [],
    failures: [],
    capabilities: /* @__PURE__ */ new Set(),
    activities: createRegistry({ onDuplicate: "retain" }),
    pages: createRegistry(),
    navigation: createRegistry(),
    slots: createRegistry(),
    commands: createRegistry()
  };
}
async function loadExtensions(state, urls, ctxFactory) {
  const disposables = [];
  for (const item of urls) {
    const scope = new DisposableScope();
    try {
      const extensionUrl = new URL(item.url, globalThis.location?.href).href;
      const mod = await import(
        /* @vite-ignore */
        extensionUrl
      );
      const result = await mod.default.setup(ownedExtensionContext(ctxFactory(state), scope));
      if (result && typeof result.dispose === "function") scope.own(result);
      validateShellState(state);
      disposables.push(scope);
      state.extensions.push(item.id);
    } catch (error) {
      let cleanupError;
      try {
        scope.dispose();
      } catch (cleanup) {
        cleanupError = cleanup;
      }
      state.failures.push({
        extensionId: item.id,
        message: [error, cleanupError].filter((failure) => failure !== void 0).map((failure) => failure instanceof Error ? failure.message : String(failure)).join("; ")
      });
    }
  }
  return disposables;
}
function ownedRegistry(registry, scope) {
  return {
    register(item) {
      return scope.own(registry.register(item));
    },
    list() {
      return registry.list();
    }
  };
}
function ownedExtensionContext(context, scope) {
  return {
    activities: ownedRegistry(context.activities, scope),
    pages: ownedRegistry(context.pages, scope),
    navigation: ownedRegistry(context.navigation, scope),
    slots: ownedRegistry(context.slots, scope),
    commands: ownedRegistry(context.commands, scope),
    rpc: context.rpc,
    events: {
      subscribe(topic, handler, requiredCapability) {
        return scope.own(context.events.subscribe(topic, handler, requiredCapability));
      }
    }
  };
}
var WebShellRuntime = class {
  state = createShellState();
  bridge;
  protocolVersion;
  extensionDisposables = [];
  helloAck = null;
  constructor(options) {
    const { bridgeUrl, protocolVersion = "1.0.0", ...bridgeOptions } = options;
    this.bridge = new WebBridgeClient(bridgeUrl, bridgeOptions);
    this.protocolVersion = protocolVersion;
  }
  get connectionState() {
    return this.bridge.state;
  }
  async connect() {
    const ack = await this.bridge.connect(this.protocolVersion);
    this.helloAck = ack;
    this.state.capabilities = new Set(ack.session.capabilities);
    return ack;
  }
  get session() {
    return this.helloAck?.session ?? null;
  }
  configureActivities(activities) {
    for (const activity of activities) this.state.activities.register(activity);
  }
  configureWebUiSettings(theme, activityId = "settings") {
    this.state.pages.register({
      id: "webui.settings",
      path: "/settings",
      title: "\u5916\u89C2",
      component: { mount: (element) => mountWebUiSettings(element, theme) }
    });
    this.state.navigation.register({
      id: "webui.settings.nav",
      activityId,
      pageId: "webui.settings",
      label: "\u5916\u89C2",
      order: 1e3
    });
  }
  async load(urls) {
    const loaded = await loadExtensions(
      this.state,
      urls,
      () => createExtensionContext(this.state, this.bridge, this.bridge)
    );
    this.extensionDisposables.push(...loaded);
    const finalized = finalizePluginActivity(this.state);
    if (finalized) this.extensionDisposables.push(finalized);
    validateShellState(this.state);
  }
  dispose() {
    for (const disposable of this.extensionDisposables.splice(0).reverse()) {
      disposable.dispose();
    }
    this.bridge.close();
  }
};
function createWebShellRuntime(options) {
  return new WebShellRuntime(options);
}
function createExtensionContext(state, rpc, events) {
  return {
    activities: state.activities,
    pages: state.pages,
    navigation: state.navigation,
    slots: state.slots,
    commands: state.commands,
    rpc,
    events
  };
}
var PLUGIN_ACTIVITY_ID = "plugins";
var PLUGIN_HOME_SLOT = "plugin.home";
function finalizePluginActivity(state) {
  if (!state.activities.list().some((item) => item.id === PLUGIN_ACTIVITY_ID)) return null;
  const home = state.slots.list().find((item) => item.slot === PLUGIN_HOME_SLOT);
  const factory = home?.component;
  if (typeof factory?.mount !== "function") return null;
  const pages = new Map(state.pages.list().map((page) => [page.id, page]));
  const covered = /* @__PURE__ */ new Set();
  for (const nav of state.navigation.list()) {
    if (nav.activityId !== PLUGIN_ACTIVITY_ID) continue;
    for (const pluginId of pluginIdsOf(pages.get(nav.pageId))) covered.add(pluginId);
  }
  const discovered = /* @__PURE__ */ new Map();
  const remember = (pluginId, label, requiredCapability) => {
    if (!covered.has(pluginId) && !discovered.has(pluginId)) {
      discovered.set(pluginId, { label, requiredCapability });
    }
  };
  for (const page of state.pages.list()) {
    for (const pluginId of pluginIdsOf(page)) {
      remember(pluginId, page.title || pluginId, page.requiredCapability);
    }
  }
  for (const slot of state.slots.list()) {
    if (slot.slot === PLUGIN_HOME_SLOT) continue;
    for (const pluginId of pluginIdsOf(slot)) {
      remember(pluginId, pluginId, slot.requiredCapability);
    }
  }
  const scope = new DisposableScope();
  let order = 1e3;
  for (const [pluginId, meta] of discovered) {
    const path = `/plugins/${pluginId}`;
    if (pages.has(pluginId) || [...pages.values()].some((page2) => page2.path === path)) continue;
    const page = {
      id: pluginId,
      path,
      title: meta.label,
      pluginId,
      requiredCapability: meta.requiredCapability,
      component: {
        mount(element) {
          return factory.mount?.(element, { pluginId });
        }
      }
    };
    scope.own(state.pages.register(page));
    scope.own(
      state.navigation.register({
        id: `${pluginId}.nav`,
        activityId: PLUGIN_ACTIVITY_ID,
        pageId: pluginId,
        label: meta.label,
        order,
        requiredCapability: meta.requiredCapability
      })
    );
    order += 1;
    pages.set(pluginId, page);
  }
  return scope;
}
function validateShellState(state) {
  const activities = new Set(state.activities.list().map((item) => item.id));
  const pages = new Map(state.pages.list().map((item) => [item.id, item]));
  const paths = /* @__PURE__ */ new Set();
  for (const page of pages.values()) {
    if (!/^\/[a-z0-9][a-z0-9._/-]*$/i.test(page.path)) {
      throw new Error(`invalid page path: ${page.path}`);
    }
    if (paths.has(page.path)) throw new Error(`duplicate page path: ${page.path}`);
    paths.add(page.path);
  }
  for (const item of state.navigation.list()) {
    if (!activities.has(item.activityId)) {
      throw new Error(`navigation ${item.id} references missing activity ${item.activityId}`);
    }
    if (!pages.has(item.pageId)) {
      throw new Error(`navigation ${item.id} references missing page ${item.pageId}`);
    }
  }
}
function mountWebShell(root, runtime, options = {}) {
  validateShellState(runtime.state);
  const state = runtime.state;
  const canUse = (required) => !required || state.capabilities.has("*") || state.capabilities.has(required);
  const pageById = new Map(
    state.pages.list().filter((page) => canUse(page.requiredCapability)).map((page) => [page.id, page])
  );
  const navItems = state.navigation.list().filter((item) => canUse(item.requiredCapability) && pageById.has(item.pageId));
  const navByActivity = /* @__PURE__ */ new Map();
  for (const item of navItems) {
    const list = navByActivity.get(item.activityId) ?? [];
    list.push(item);
    navByActivity.set(item.activityId, list);
  }
  for (const list of navByActivity.values()) {
    list.sort((a, b) => (a.order ?? 0) - (b.order ?? 0) || a.label.localeCompare(b.label));
  }
  const isEnabled = (item) => !item.disabled;
  const activities = state.activities.list().filter((item) => navByActivity.get(item.id)?.some(isEnabled)).sort((a, b) => (a.order ?? 0) - (b.order ?? 0) || a.label.localeCompare(b.label));
  if (!activities.length) throw new Error("web shell has no accessible pages");
  root.innerHTML = `<div class="mutsuki-console mutsuki-console--activity-shell">
    <nav class="console-activity" aria-label="\u5DE5\u4F5C\u57DF">
      <div class="console-activity__top"></div>
      <div class="console-activity__spacer"></div>
      <div class="console-activity__bottom"></div>
    </nav>
    <aside class="console-context" aria-label="\u9875\u9762\u5BFC\u822A">
      <div class="console-context__brand"></div>
      <div class="console-context__title"></div>
      <nav class="console-context__nav"></nav>
    </aside>
    <button class="console-context-mask" type="button" aria-label="\u5173\u95ED\u5BFC\u822A"></button>
    <main class="console-main">
      <header class="console-page-header">
        <button class="console-menu-toggle" type="button" aria-label="\u6253\u5F00\u5BFC\u822A">\u2630</button>
        <h1></h1>
        <div class="console-page-header__actions">
          <span class="console-connection-dot"></span>
          <span class="console-connection-label"></span>
        </div>
      </header>
      <section class="console-page-content" id="content"></section>
    </main>
  </div>`;
  const shell = root.firstElementChild;
  const activityTop = shell.querySelector(".console-activity__top");
  const activityBottom = shell.querySelector(".console-activity__bottom");
  const contextTitle = shell.querySelector(".console-context__title");
  const contextNav = shell.querySelector(".console-context__nav");
  const pageTitle = shell.querySelector(".console-page-header h1");
  const headerActions = shell.querySelector(".console-page-header__actions");
  const content = shell.querySelector(".console-page-content");
  const menuToggle = shell.querySelector(".console-menu-toggle");
  const mask = shell.querySelector(".console-context-mask");
  shell.querySelector(".console-context__brand").textContent = options.brand ?? "Mutsuki";
  let activeActivityId = "";
  let activePageId = "";
  let mountedPage = null;
  let mountGeneration = 0;
  const closeContext = () => shell.classList.remove("is-context-open");
  const routeFor = (activityId, pageId) => `#/${activityId}/${pageId}`;
  const route = () => {
    const match = location.hash.match(/^#\/([^/]+)\/([^/]+)$/);
    return match ? { activityId: decodeURIComponent(match[1]), pageId: decodeURIComponent(match[2]) } : null;
  };
  const firstRoute = () => {
    const preferred = options.homePageId && navItems.find((item2) => item2.pageId === options.homePageId && !item2.disabled);
    const item = preferred ?? navByActivity.get(activities[0].id)?.find(isEnabled);
    if (!item) throw new Error("web shell has no accessible navigation item");
    return { activityId: item.activityId, pageId: item.pageId };
  };
  const normalizeRoute = () => {
    const current = route();
    if (current && navByActivity.get(current.activityId)?.some((item) => isEnabled(item) && item.pageId === current.pageId)) {
      return current;
    }
    const fallback = firstRoute();
    history.replaceState({}, "", routeFor(fallback.activityId, fallback.pageId));
    return fallback;
  };
  const drawActivities = () => {
    activityTop.replaceChildren();
    activityBottom.replaceChildren();
    for (const activity of activities) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = `console-activity__item${activity.id === activeActivityId ? " is-active" : ""}`;
      button.title = activity.label;
      button.setAttribute("aria-label", activity.label);
      button.setAttribute("aria-pressed", activity.id === activeActivityId ? "true" : "false");
      button.innerHTML = iconMarkup(activity.icon, activity.label);
      button.onclick = () => {
        const target = navByActivity.get(activity.id)?.find(isEnabled);
        if (target) location.hash = routeFor(activity.id, target.pageId).slice(1);
      };
      (activity.position === "bottom" ? activityBottom : activityTop).append(button);
    }
  };
  const drawContext = () => {
    const activity = activities.find((item) => item.id === activeActivityId);
    const items = navByActivity.get(activeActivityId) ?? [];
    const showContext = items.length > 1;
    shell.classList.toggle("is-context-hidden", !showContext);
    if (!showContext) closeContext();
    contextTitle.textContent = activity?.label ?? "";
    contextNav.replaceChildren();
    if (!showContext) return;
    for (const section of groupNavigationItems(navByActivity.get(activeActivityId) ?? [])) {
      if (section.group) {
        const heading = document.createElement("div");
        heading.className = "console-context__group";
        heading.textContent = section.group;
        contextNav.append(heading);
      }
      for (const item of section.items) {
        const button = document.createElement("button");
        button.type = "button";
        button.className = `sb-tree__row lilia-interactive-item${item.pageId === activePageId ? " is-active" : ""}${item.disabled ? " sb-tree__row--muted" : ""}`;
        button.textContent = item.label;
        if (item.pageId === activePageId) button.setAttribute("aria-current", "page");
        if (item.disabled) {
          button.disabled = true;
          button.setAttribute("aria-disabled", "true");
        } else {
          button.onclick = () => {
            location.hash = routeFor(activeActivityId, item.pageId).slice(1);
            closeContext();
          };
        }
        contextNav.append(button);
      }
    }
  };
  const renderRoute = async () => {
    const next = normalizeRoute();
    activeActivityId = next.activityId;
    activePageId = next.pageId;
    drawActivities();
    drawContext();
    mountedPage?.dispose();
    mountedPage = null;
    const generation = ++mountGeneration;
    const page = pageById.get(activePageId);
    if (!page) return;
    pageTitle.textContent = page.title;
    document.title = `${page.title} | ${options.brand ?? "Mutsuki"}`;
    content.className = "console-page-content";
    content.replaceChildren();
    content.setAttribute("aria-busy", "true");
    const pageHost = document.createElement("div");
    pageHost.className = "page-body";
    content.append(pageHost);
    try {
      const disposable = await page.component.mount(pageHost);
      if (generation !== mountGeneration) disposable?.dispose();
      else mountedPage = disposable ?? null;
    } catch (error) {
      if (generation === mountGeneration) {
        content.textContent = error instanceof Error ? error.message : String(error);
        content.className = "console-page-content error-banner";
      }
    } finally {
      if (generation === mountGeneration) content.removeAttribute("aria-busy");
    }
  };
  const legacyPage = new URLSearchParams(location.search).get("page");
  if (!location.hash && legacyPage) {
    const legacy = navItems.find(
      (item) => !item.disabled && pageById.get(item.pageId)?.path.replace(/^\//, "") === legacyPage
    );
    if (legacy) history.replaceState({}, "", routeFor(legacy.activityId, legacy.pageId));
  }
  const hashListener = () => void renderRoute();
  window.addEventListener("hashchange", hashListener);
  menuToggle.onclick = () => shell.classList.toggle("is-context-open");
  mask.onclick = closeContext;
  const connectionLabel = shell.querySelector(".console-connection-label");
  const connectionDot = shell.querySelector(".console-connection-dot");
  if (state.failures.length) {
    const retry = document.createElement("button");
    retry.type = "button";
    retry.className = "console-extension-retry";
    retry.textContent = `\u91CD\u8BD5 ${state.failures.length} \u4E2A\u9875\u9762`;
    retry.onclick = () => location.reload();
    headerActions.append(retry);
  }
  const connection = runtime.bridge.onStateChange((value) => {
    const labels = {
      idle: "\u672A\u8FDE\u63A5",
      connecting: "\u8FDE\u63A5\u4E2D",
      open: "\u5DF2\u8FDE\u63A5",
      reconnecting: "\u91CD\u65B0\u8FDE\u63A5\u4E2D",
      closed: "\u5DF2\u65AD\u5F00"
    };
    const safeMode = !!runtime.session?.safe_mode;
    const quiet = value === "open" && !safeMode;
    connectionLabel.hidden = quiet;
    connectionDot.hidden = quiet;
    connectionLabel.textContent = safeMode ? `${labels[value]} \xB7 \u5B89\u5168\u6A21\u5F0F` : labels[value];
    connectionDot.dataset.state = value;
  });
  void renderRoute();
  return {
    dispose() {
      mountGeneration += 1;
      mountedPage?.dispose();
      connection.dispose();
      window.removeEventListener("hashchange", hashListener);
      root.replaceChildren();
    }
  };
}
function mountWebUiSettings(host, theme) {
  host.className = "page-body settings-page";
  host.innerHTML = `<section class="card">
    <h2>\u5916\u89C2</h2>
    <div class="settings-row">
      <div class="settings-row__label">
        <strong>\u4E3B\u9898</strong>
      </div>
      <div class="settings-row__control segmented" role="group" aria-label="\u4E3B\u9898">
        <button type="button" data-theme="system">\u8DDF\u968F\u7CFB\u7EDF</button>
        <button type="button" data-theme="light">\u6D45\u8272</button>
        <button type="button" data-theme="dark">\u6DF1\u8272</button>
      </div>
    </div>
  </section>`;
  const draw = () => {
    for (const button of host.querySelectorAll("[data-theme]")) {
      const active = button.dataset.theme === theme.preference;
      button.classList.toggle("is-active", active);
      button.setAttribute("aria-pressed", String(active));
    }
  };
  for (const button of host.querySelectorAll("[data-theme]")) {
    button.onclick = () => {
      const value = button.dataset.theme;
      if (isThemePreference(value)) {
        theme.setPreference(value);
        draw();
      }
    };
  }
  draw();
}
function iconMarkup(icon, label) {
  const paths = {
    home: '<path d="M3 11.5 12 4l9 7.5V21h-6v-6H9v6H3z"/>',
    bot: '<path d="M7 8h10a4 4 0 0 1 4 4v5a4 4 0 0 1-4 4H7a4 4 0 0 1-4-4v-5a4 4 0 0 1 4-4Zm2 5v2m6-2v2M12 8V4m-2 0h4"/>',
    flow: '<path d="M5 4h5v5H5zM14 15h5v5h-5zM7.5 9v3a5 5 0 0 0 5 5H14M14 4h5v5h-5z"/>',
    system: '<path d="M12 3v3m0 12v3M3 12h3m12 0h3M5.6 5.6l2.1 2.1m8.6 8.6 2.1 2.1m0-12.8-2.1 2.1m-8.6 8.6-2.1 2.1M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6Z"/>',
    config: '<path d="M5 4h14v5H5zM5 13h14v7H5zM8 6.5h4M8 16h8"/>',
    settings: '<path d="M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8Zm8 4 2-1-2-3-2 .4-1.4-1.4.4-2-3-2-1 2h-2l-1-2-3 2 .4 2L6 8.4 4 8l-2 3 2 1v2l-2 1 2 3 2-.4L7.4 19 7 21l3 2 1-2h2l1 2 3-2-.4-2 1.4-1.4 2 .4 2-3-2-1Z"/>',
    sandbox: '<path d="M4 8h16v11a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V8Zm0 0 8-4 8 4M8 12h8M8 16h5"/>'
  };
  const path = paths[icon];
  if (!path) return `<span aria-hidden="true">${escapeMarkup(label.slice(0, 1))}</span>`;
  return `<svg aria-hidden="true" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">${path}</svg>`;
}
function escapeMarkup(value) {
  return value.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;");
}
var SHARED_IMPORT_MAP = {
  vue: "/shared/vue.js",
  "vue-router": "/shared/vue-router.js",
  pinia: "/shared/pinia.js",
  "@mutsuki/web-sdk": "/shared/web-sdk.js",
  "@mutsuki/ui": "/shared/ui.js"
};
export {
  PLUGIN_ACTIVITY_ID,
  PLUGIN_HOME_SLOT,
  SHARED_IMPORT_MAP,
  WebShellRuntime,
  createExtensionContext,
  createShellState,
  createWebShellRuntime,
  createWebUiThemeController,
  finalizePluginActivity,
  groupNavigationItems,
  loadExtensions,
  mountWebShell,
  validateShellState
};
