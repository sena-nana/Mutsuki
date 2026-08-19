import {
  WebBridgeClient,
  DisposableScope,
  createRegistry,
  type ActivityRegistration,
  type BridgeConnectionState,
  type BridgeHelloAck,
  type Disposable,
  type ExtensionContext,
  type NavigationRegistration,
  type PageRegistration,
  type Registry,
  type WebBridgeClientOptions,
  type WebExtension,
} from "@mutsuki/web-sdk";
import { applyTheme } from "@mutsuki/ui";

export type WebUiThemePreference = "system" | "light" | "dark";

export interface WebUiThemeController extends Disposable {
  readonly preference: WebUiThemePreference;
  setPreference(preference: WebUiThemePreference): void;
}

export interface WebUiThemeControllerOptions {
  storage?: Pick<Storage, "getItem" | "setItem">;
  media?: Pick<MediaQueryList, "matches" | "addEventListener" | "removeEventListener">;
  storageKey?: string;
}

const WEB_UI_THEME_STORAGE_KEY = "mutsuki.webui.theme";

export function createWebUiThemeController(
  options: WebUiThemeControllerOptions = {},
): WebUiThemeController {
  const storage = options.storage ?? globalThis.localStorage;
  const media = options.media ?? globalThis.matchMedia?.("(prefers-color-scheme: light)");
  const storageKey = options.storageKey ?? WEB_UI_THEME_STORAGE_KEY;
  const stored = storage?.getItem(storageKey);
  let preference: WebUiThemePreference = isThemePreference(stored) ? stored : "system";
  let disposed = false;

  const apply = () => applyTheme(
    preference === "system" ? (media?.matches ? "light" : "dark") : preference,
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
    },
  };
}

function isThemePreference(value: string | null | undefined): value is WebUiThemePreference {
  return value === "system" || value === "light" || value === "dark";
}

export function groupNavigationItems(
  items: NavigationRegistration[],
): Array<{ group?: string; items: NavigationRegistration[] }> {
  const sections: Array<{ group?: string; items: NavigationRegistration[] }> = [];
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

export interface ShellState {
  extensions: string[];
  failures: Array<{ extensionId: string; message: string }>;
  capabilities: Set<string>;
  activities: ReturnType<typeof createRegistry<ActivityRegistration>>;
  pages: ReturnType<typeof createRegistry<PageRegistration>>;
  navigation: ReturnType<typeof createRegistry<NavigationRegistration>>;
  slots: ReturnType<typeof createRegistry<{ id: string; slot: string; component: unknown }>>;
  commands: ReturnType<
    typeof createRegistry<{ id: string; title: string; run: () => void | Promise<void> }>
  >;
}

export function createShellState(): ShellState {
  return {
    extensions: [],
    failures: [],
    capabilities: new Set(),
    activities: createRegistry(),
    pages: createRegistry(),
    navigation: createRegistry(),
    slots: createRegistry(),
    commands: createRegistry(),
  };
}

/** Load precompiled ESM extensions through import maps. No runtime Vue SFC compile. */
export async function loadExtensions(
  state: ShellState,
  urls: Array<{ id: string; url: string }>,
  ctxFactory: (state: ShellState) => ExtensionContext,
): Promise<Disposable[]> {
  const disposables: Disposable[] = [];
  for (const item of urls) {
    const scope = new DisposableScope();
    try {
      const extensionUrl = new URL(item.url, globalThis.location?.href).href;
      const mod = (await import(/* @vite-ignore */ extensionUrl)) as {
        default: WebExtension;
      };
      const result = await mod.default.setup(ownedExtensionContext(ctxFactory(state), scope));
      if (result && typeof result.dispose === "function") scope.own(result);
      validateShellState(state);
      disposables.push(scope);
      state.extensions.push(item.id);
    } catch (error) {
      let cleanupError: unknown;
      try {
        scope.dispose();
      } catch (cleanup) {
        cleanupError = cleanup;
      }
      state.failures.push({
        extensionId: item.id,
        message: [error, cleanupError]
          .filter((failure) => failure !== undefined)
          .map((failure) => (failure instanceof Error ? failure.message : String(failure)))
          .join("; "),
      });
    }
  }
  return disposables;
}

function ownedRegistry<T>(registry: Registry<T>, scope: DisposableScope): Registry<T> {
  return {
    register(item) {
      return scope.own(registry.register(item));
    },
    list() {
      return registry.list();
    },
  };
}

function ownedExtensionContext(
  context: ExtensionContext,
  scope: DisposableScope,
): ExtensionContext {
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
      },
    },
  };
}

export interface WebShellRuntimeOptions extends WebBridgeClientOptions {
  bridgeUrl: string;
  protocolVersion?: string;
}

/** Owns the single authenticated bridge shared by every loaded extension. */
export class WebShellRuntime implements Disposable {
  readonly state = createShellState();
  readonly bridge: WebBridgeClient;
  private readonly protocolVersion: string;
  private extensionDisposables: Disposable[] = [];
  private helloAck: BridgeHelloAck | null = null;

  constructor(options: WebShellRuntimeOptions) {
    const { bridgeUrl, protocolVersion = "1.0.0", ...bridgeOptions } = options;
    this.bridge = new WebBridgeClient(bridgeUrl, bridgeOptions);
    this.protocolVersion = protocolVersion;
  }

  get connectionState(): BridgeConnectionState {
    return this.bridge.state;
  }

  async connect(): Promise<BridgeHelloAck> {
    const ack = await this.bridge.connect(this.protocolVersion);
    this.helloAck = ack;
    this.state.capabilities = new Set(ack.session.capabilities);
    return ack;
  }

  get session(): BridgeHelloAck["session"] | null {
    return this.helloAck?.session ?? null;
  }

  configureActivities(activities: ActivityRegistration[]): void {
    for (const activity of activities) this.state.activities.register(activity);
  }

  configureWebUiSettings(
    theme: WebUiThemeController,
    activityId = "settings",
  ): void {
    this.state.pages.register({
      id: "webui.settings",
      path: "/settings",
      title: "外观",
      component: { mount: (element) => mountWebUiSettings(element, theme) },
    });
    this.state.navigation.register({
      id: "webui.settings.nav",
      activityId,
      pageId: "webui.settings",
      label: "外观",
      order: 10,
    });
  }

  async load(urls: Array<{ id: string; url: string }>): Promise<void> {
    const loaded = await loadExtensions(this.state, urls, () =>
      createExtensionContext(this.state, this.bridge, this.bridge),
    );
    this.extensionDisposables.push(...loaded);
  }

  dispose(): void {
    for (const disposable of this.extensionDisposables.splice(0).reverse()) {
      disposable.dispose();
    }
    this.bridge.close();
  }
}

export function createWebShellRuntime(options: WebShellRuntimeOptions): WebShellRuntime {
  return new WebShellRuntime(options);
}

export function createExtensionContext(
  state: ShellState,
  rpc: ExtensionContext["rpc"],
  events: ExtensionContext["events"],
): ExtensionContext {
  return {
    activities: state.activities,
    pages: state.pages,
    navigation: state.navigation,
    slots: state.slots,
    commands: state.commands,
    rpc,
    events,
  };
}

export function validateShellState(state: ShellState): void {
  const activities = new Set(state.activities.list().map((item) => item.id));
  const pages = new Map(state.pages.list().map((item) => [item.id, item]));
  const paths = new Set<string>();
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

export interface WebShellMountOptions {
  brand?: string;
  homePageId?: string;
}

export function mountWebShell(
  root: HTMLElement,
  runtime: WebShellRuntime,
  options: WebShellMountOptions = {},
): Disposable {
  validateShellState(runtime.state);
  const state = runtime.state;
  const canUse = (required?: string) =>
    !required || state.capabilities.has("*") || state.capabilities.has(required);
  const pageById = new Map(
    state.pages.list().filter((page) => canUse(page.requiredCapability)).map((page) => [page.id, page]),
  );
  const navItems = state.navigation
    .list()
    .filter((item) => canUse(item.requiredCapability) && pageById.has(item.pageId));
  const navByActivity = new Map<string, NavigationRegistration[]>();
  for (const item of navItems) {
    const list = navByActivity.get(item.activityId) ?? [];
    list.push(item);
    navByActivity.set(item.activityId, list);
  }
  for (const list of navByActivity.values()) {
    list.sort((a, b) => (a.order ?? 0) - (b.order ?? 0) || a.label.localeCompare(b.label));
  }
  const activities = state.activities
    .list()
    .filter((item) => navByActivity.has(item.id))
    .sort((a, b) => (a.order ?? 0) - (b.order ?? 0) || a.label.localeCompare(b.label));
  if (!activities.length) throw new Error("web shell has no accessible pages");

  root.innerHTML = `<div class="mutsuki-console mutsuki-console--activity-shell">
    <nav class="console-activity" aria-label="工作域">
      <div class="console-activity__top"></div>
      <div class="console-activity__spacer"></div>
      <div class="console-activity__bottom"></div>
    </nav>
    <aside class="console-context" aria-label="页面导航">
      <div class="console-context__brand"></div>
      <div class="console-context__title"></div>
      <nav class="console-context__nav"></nav>
    </aside>
    <button class="console-context-mask" type="button" aria-label="关闭导航"></button>
    <main class="console-main">
      <header class="console-page-header">
        <button class="console-menu-toggle" type="button" aria-label="打开导航">☰</button>
        <h1></h1>
        <div class="console-page-header__actions">
          <span class="console-connection-dot"></span>
          <span class="console-connection-label"></span>
        </div>
      </header>
      <section class="console-page-content" id="content"></section>
    </main>
  </div>`;
  const shell = root.firstElementChild as HTMLElement;
  const activityTop = shell.querySelector(".console-activity__top") as HTMLElement;
  const activityBottom = shell.querySelector(".console-activity__bottom") as HTMLElement;
  const contextTitle = shell.querySelector(".console-context__title") as HTMLElement;
  const contextNav = shell.querySelector(".console-context__nav") as HTMLElement;
  const pageTitle = shell.querySelector(".console-page-header h1") as HTMLElement;
  const headerActions = shell.querySelector(".console-page-header__actions") as HTMLElement;
  const content = shell.querySelector(".console-page-content") as HTMLElement;
  const menuToggle = shell.querySelector(".console-menu-toggle") as HTMLButtonElement;
  const mask = shell.querySelector(".console-context-mask") as HTMLButtonElement;
  (shell.querySelector(".console-context__brand") as HTMLElement).textContent = options.brand ?? "Mutsuki";

  let activeActivityId = "";
  let activePageId = "";
  let mountedPage: Disposable | null = null;
  let mountGeneration = 0;

  const closeContext = () => shell.classList.remove("is-context-open");
  const routeFor = (activityId: string, pageId: string) => `#/${activityId}/${pageId}`;
  const route = () => {
    const match = location.hash.match(/^#\/([^/]+)\/([^/]+)$/);
    return match ? { activityId: decodeURIComponent(match[1]), pageId: decodeURIComponent(match[2]) } : null;
  };
  const firstRoute = () => {
    const preferred = options.homePageId && navItems.find((item) => item.pageId === options.homePageId);
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
        button.className = `sb-tree__row lilia-interactive-item${item.pageId === activePageId ? " is-active" : ""}`;
        button.textContent = item.label;
        if (item.pageId === activePageId) button.setAttribute("aria-current", "page");
        button.onclick = () => {
          location.hash = routeFor(activeActivityId, item.pageId).slice(1);
          closeContext();
        };
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
    const legacy = navItems.find((item) => pageById.get(item.pageId)?.path.replace(/^\//, "") === legacyPage);
    if (legacy) history.replaceState({}, "", routeFor(legacy.activityId, legacy.pageId));
  }
  const hashListener = () => void renderRoute();
  window.addEventListener("hashchange", hashListener);
  menuToggle.onclick = () => shell.classList.toggle("is-context-open");
  mask.onclick = closeContext;
  const connectionLabel = shell.querySelector(".console-connection-label") as HTMLElement;
  const connectionDot = shell.querySelector(".console-connection-dot") as HTMLElement;
  if (state.failures.length) {
    const retry = document.createElement("button");
    retry.type = "button";
    retry.className = "console-extension-retry";
    retry.textContent = `重试 ${state.failures.length} 个页面`;
    retry.onclick = () => location.reload();
    headerActions.append(retry);
  }
  const connection = runtime.bridge.onStateChange((value) => {
    const labels: Record<BridgeConnectionState, string> = {
      idle: "未连接", connecting: "连接中", open: "已连接", reconnecting: "重新连接中", closed: "已断开",
    };
    connectionLabel.textContent = runtime.session?.safe_mode ? `${labels[value]} · 安全模式` : labels[value];
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
    },
  };
}

function mountWebUiSettings(
  host: HTMLElement,
  theme: WebUiThemeController,
): void {
  host.className = "page-body settings-page";
  host.innerHTML = `<section class="card">
    <h2>外观</h2>
    <div class="settings-row">
      <div class="settings-row__label">
        <strong>主题</strong>
      </div>
      <div class="settings-row__control segmented" role="group" aria-label="主题">
        <button type="button" data-theme="system">跟随系统</button>
        <button type="button" data-theme="light">浅色</button>
        <button type="button" data-theme="dark">深色</button>
      </div>
    </div>
  </section>`;
  const draw = () => {
    for (const button of host.querySelectorAll<HTMLButtonElement>("[data-theme]")) {
      const active = button.dataset.theme === theme.preference;
      button.classList.toggle("is-active", active);
      button.setAttribute("aria-pressed", String(active));
    }
  };
  for (const button of host.querySelectorAll<HTMLButtonElement>("[data-theme]")) {
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

function iconMarkup(icon: string, label: string): string {
  const paths: Record<string, string> = {
    home: '<path d="M3 11.5 12 4l9 7.5V21h-6v-6H9v6H3z"/>',
    bot: '<path d="M7 8h10a4 4 0 0 1 4 4v5a4 4 0 0 1-4 4H7a4 4 0 0 1-4-4v-5a4 4 0 0 1 4-4Zm2 5v2m6-2v2M12 8V4m-2 0h4"/>',
    flow: '<path d="M5 4h5v5H5zM14 15h5v5h-5zM7.5 9v3a5 5 0 0 0 5 5H14M14 4h5v5h-5z"/>',
    system: '<path d="M12 3v3m0 12v3M3 12h3m12 0h3M5.6 5.6l2.1 2.1m8.6 8.6 2.1 2.1m0-12.8-2.1 2.1m-8.6 8.6-2.1 2.1M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6Z"/>',
    config: '<path d="M5 4h14v5H5zM5 13h14v7H5zM8 6.5h4M8 16h8"/>',
    settings: '<path d="M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8Zm8 4 2-1-2-3-2 .4-1.4-1.4.4-2-3-2-1 2h-2l-1-2-3 2 .4 2L6 8.4 4 8l-2 3 2 1v2l-2 1 2 3 2-.4L7.4 19 7 21l3 2 1-2h2l1 2 3-2-.4-2 1.4-1.4 2 .4 2-3-2-1Z"/>',
    sandbox: '<path d="M4 8h16v11a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V8Zm0 0 8-4 8 4M8 12h8M8 16h5"/>',
  };
  const path = paths[icon];
  if (!path) return `<span aria-hidden="true">${escapeMarkup(label.slice(0, 1))}</span>`;
  return `<svg aria-hidden="true" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">${path}</svg>`;
}

function escapeMarkup(value: string): string {
  return value.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;");
}

export const SHARED_IMPORT_MAP = {
  vue: "/shared/vue.js",
  "vue-router": "/shared/vue-router.js",
  pinia: "/shared/pinia.js",
  "@mutsuki/web-sdk": "/shared/web-sdk.js",
  "@mutsuki/ui": "/shared/ui.js",
} as const;
