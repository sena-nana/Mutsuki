// src/runtime.ts
import {
  WebBridgeClient,
  DisposableScope,
  createRegistry
} from "./web-sdk.js";
function createShellState() {
  return {
    extensions: [],
    failures: [],
    capabilities: /* @__PURE__ */ new Set(),
    activities: createRegistry(),
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
  async load(urls) {
    const loaded = await loadExtensions(
      this.state,
      urls,
      () => createExtensionContext(this.state, this.bridge, this.bridge)
    );
    this.extensionDisposables.push(...loaded);
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
function validateShellState(state) {
  const activities = new Set(state.activities.list().map((item) => item.id));
  const pages = new Map(state.pages.list().map((item) => [item.id, item]));
  const paths = /* @__PURE__ */ new Set();
  for (const page of pages.values()) {
    if (!/^\/[a-z0-9][a-z0-9-/]*$/i.test(page.path)) {
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
  const activities = state.activities.list().filter((item) => navByActivity.has(item.id)).sort((a, b) => (a.order ?? 0) - (b.order ?? 0) || a.label.localeCompare(b.label));
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
      <div class="console-context__footer"><span class="console-connection-dot"></span><span class="console-connection-label"></span></div>
    </aside>
    <button class="console-context-mask" type="button" aria-label="\u5173\u95ED\u5BFC\u822A"></button>
    <main class="console-main">
      <header class="console-page-header"><button class="console-menu-toggle" type="button" aria-label="\u6253\u5F00\u5BFC\u822A">\u2630</button><h1></h1></header>
      <section class="console-page-content" id="content"></section>
    </main>
  </div>`;
  const shell = root.firstElementChild;
  const activityTop = shell.querySelector(".console-activity__top");
  const activityBottom = shell.querySelector(".console-activity__bottom");
  const contextTitle = shell.querySelector(".console-context__title");
  const contextNav = shell.querySelector(".console-context__nav");
  const pageTitle = shell.querySelector(".console-page-header h1");
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
    const preferred = options.homePageId && navItems.find((item2) => item2.pageId === options.homePageId);
    const item = preferred ?? navByActivity.get(activities[0].id)?.[0];
    if (!item) throw new Error("web shell has no accessible navigation item");
    return { activityId: item.activityId, pageId: item.pageId };
  };
  const normalizeRoute = () => {
    const current = route();
    if (current && navByActivity.get(current.activityId)?.some((item) => item.pageId === current.pageId)) {
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
        const target = navByActivity.get(activity.id)?.[0];
        if (target) location.hash = routeFor(activity.id, target.pageId).slice(1);
      };
      (activity.position === "bottom" ? activityBottom : activityTop).append(button);
    }
  };
  const drawContext = () => {
    const activity = activities.find((item) => item.id === activeActivityId);
    contextTitle.textContent = activity?.label ?? "";
    contextNav.replaceChildren();
    for (const item of navByActivity.get(activeActivityId) ?? []) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = `sb-tree__row lilia-interactive-item${item.pageId === activePageId ? " is-active" : ""}`;
      button.textContent = item.label;
      if (item.pageId === activePageId) button.setAttribute("aria-current", "page");
      button.onclick = () => {
        location.hash = routeFor(activeActivityId, item.pageId).slice(1);
        closeContext();
      };
      contextNav.append(button);
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
    const legacy = navItems.find((item) => pageById.get(item.pageId)?.path.replace(/^\//, "") === legacyPage);
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
    shell.querySelector(".console-context__footer")?.append(retry);
  }
  const connection = runtime.bridge.onStateChange((value) => {
    const labels = {
      idle: "\u672A\u8FDE\u63A5",
      connecting: "\u8FDE\u63A5\u4E2D",
      open: "\u5DF2\u8FDE\u63A5",
      reconnecting: "\u91CD\u65B0\u8FDE\u63A5\u4E2D",
      closed: "\u5DF2\u65AD\u5F00"
    };
    connectionLabel.textContent = runtime.session?.safe_mode ? `${labels[value]} \xB7 \u5B89\u5168\u6A21\u5F0F` : labels[value];
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
function iconMarkup(icon, label) {
  const paths = {
    home: '<path d="M3 11.5 12 4l9 7.5V21h-6v-6H9v6H3z"/>',
    bot: '<path d="M7 8h10a4 4 0 0 1 4 4v5a4 4 0 0 1-4 4H7a4 4 0 0 1-4-4v-5a4 4 0 0 1 4-4Zm2 5v2m6-2v2M12 8V4m-2 0h4"/>',
    flow: '<path d="M5 4h5v5H5zM14 15h5v5h-5zM7.5 9v3a5 5 0 0 0 5 5H14M14 4h5v5h-5z"/>',
    system: '<path d="M12 3v3m0 12v3M3 12h3m12 0h3M5.6 5.6l2.1 2.1m8.6 8.6 2.1 2.1m0-12.8-2.1 2.1m-8.6 8.6-2.1 2.1M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6Z"/>',
    settings: '<path d="M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8Zm8 4 2-1-2-3-2 .4-1.4-1.4.4-2-3-2-1 2h-2l-1-2-3 2 .4 2L6 8.4 4 8l-2 3 2 1v2l-2 1 2 3 2-.4L7.4 19 7 21l3 2 1-2h2l1 2 3-2-.4-2 1.4-1.4 2 .4 2-3-2-1Z"/>'
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
  SHARED_IMPORT_MAP,
  WebShellRuntime,
  createExtensionContext,
  createShellState,
  createWebShellRuntime,
  loadExtensions,
  mountWebShell,
  validateShellState
};
