import {
  MAX_EXTENSIONS,
  WEB_PROTOCOL_VERSION,
  WebBridgeClient,
  DisposableScope,
  createRegistry,
  pluginDisplayName,
  pluginIdsOf,
  type ActivityRegistration,
  type BridgeConnectionState,
  type BridgeHelloAck,
  type Disposable,
  type ExtensionContext,
  type NavigationRegistration,
  type PageRegistration,
  type Registry,
  type SlotRegistration,
  type WebBridgeClientOptions,
  type WebExtension,
} from "@mutsuki/web-sdk";
import { applyTheme } from "@mutsuki/ui";
import { defineComponent, h, markRaw, type Component } from "vue";

import type { ConsoleLayout, ShellFailure } from "./layout.js";

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
  failures: ShellFailure[];
  /** Extension that registered each page and navigation id (for failure attribution). */
  owners: Map<string, string>;
  capabilities: Set<string>;
  activities: ReturnType<typeof createRegistry<ActivityRegistration>>;
  pages: ReturnType<typeof createRegistry<PageRegistration>>;
  navigation: ReturnType<typeof createRegistry<NavigationRegistration>>;
  slots: ReturnType<typeof createRegistry<SlotRegistration>>;
  commands: ReturnType<
    typeof createRegistry<{ id: string; title: string; run: () => void | Promise<void> }>
  >;
}

export function createShellState(): ShellState {
  return {
    extensions: [],
    failures: [],
    owners: new Map(),
    capabilities: new Set(),
    activities: createRegistry({ onDuplicate: "retain" }),
    pages: createRegistry(),
    navigation: createRegistry(),
    slots: createRegistry(),
    commands: createRegistry(),
  };
}

/** One extension module and the stylesheets its manifest declares. */
export interface ExtensionModuleRef {
  id: string;
  url: string;
  styles?: string[];
}

/** Links one stylesheet once per document and waits until it applies. */
function linkStylesheet(href: string): Promise<void> {
  if (typeof document === "undefined") return Promise.resolve();
  const absolute = new URL(href, globalThis.location?.href).href;
  for (const existing of document.querySelectorAll<HTMLLinkElement>('link[rel="stylesheet"]')) {
    if (existing.href === absolute) return Promise.resolve();
  }
  return new Promise((resolve, reject) => {
    const link = document.createElement("link");
    link.rel = "stylesheet";
    link.href = absolute;
    link.addEventListener("load", () => resolve(), { once: true });
    link.addEventListener("error", () => reject(new Error(`stylesheet failed: ${href}`)), {
      once: true,
    });
    document.head.append(link);
  });
}

/** Load precompiled ESM extensions through import maps. No runtime Vue SFC compile. */
export async function loadExtensions(
  state: ShellState,
  urls: ExtensionModuleRef[],
  ctxFactory: (state: ShellState) => ExtensionContext,
): Promise<Disposable[]> {
  const disposables: Disposable[] = [];
  for (const item of urls) {
    if (state.extensions.length >= MAX_EXTENSIONS) {
      state.failures.push({
        extensionId: item.id,
        stage: "load",
        message: `extension budget of ${MAX_EXTENSIONS} exceeded`,
      });
      continue;
    }
    const scope = new DisposableScope();
    try {
      await Promise.all((item.styles ?? []).map(linkStylesheet));
      const extensionUrl = new URL(item.url, globalThis.location?.href).href;
      const mod = (await import(/* @vite-ignore */ extensionUrl)) as {
        default: WebExtension;
      };
      const result = await mod.default.setup(
        ownedExtensionContext(ctxFactory(state), scope, state, item.id),
      );
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
        stage: "load",
        message: [error, cleanupError]
          .filter((failure) => failure !== undefined)
          .map((failure) => (failure instanceof Error ? failure.message : String(failure)))
          .join("; "),
      });
    }
  }
  return disposables;
}

function ownedRegistry<T extends { id: string }>(
  registry: Registry<T>,
  scope: DisposableScope,
  onRegister?: (item: T) => void,
): Registry<T> {
  return {
    register(item) {
      const owned = scope.own(registry.register(item));
      onRegister?.(item);
      return owned;
    },
    list() {
      return registry.list();
    },
  };
}

/**
 * Activities belong to the product layout. A legacy extension that still registers one gets
 * a no-op; an activity the layout does not know is recorded as a layout failure.
 */
function layoutActivities(
  registry: Registry<ActivityRegistration>,
  state: ShellState,
  extensionId: string,
): Registry<ActivityRegistration> {
  return {
    register(item) {
      if (!registry.list().some((activity) => activity.id === item.id)) {
        state.failures.push({
          extensionId,
          stage: "layout",
          message: `activity ${item.id} is not defined by the console layout`,
        });
      }
      return { dispose() {} };
    },
    list: () => registry.list(),
  };
}

function ownedExtensionContext(
  context: ExtensionContext,
  scope: DisposableScope,
  state: ShellState,
  extensionId: string,
): ExtensionContext {
  const own = (item: { id: string }) => state.owners.set(item.id, extensionId);
  return {
    activities: layoutActivities(context.activities, state, extensionId),
    pages: ownedRegistry(context.pages, scope, own),
    navigation: ownedRegistry(context.navigation, scope, own),
    slots: ownedRegistry(context.slots, scope),
    commands: ownedRegistry(context.commands, scope),
    rpc: context.rpc,
    events: {
      subscribe(topic, handler, requiredCapability, onError) {
        return scope.own(context.events.subscribe(topic, handler, requiredCapability, onError));
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
    const { bridgeUrl, protocolVersion = WEB_PROTOCOL_VERSION, ...bridgeOptions } = options;
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

  /** Installs the product layout's activities; extensions cannot add more. */
  configureLayout(layout: ConsoleLayout): void {
    for (const activity of layout.activities) this.state.activities.register(activity);
  }

  async load(urls: ExtensionModuleRef[]): Promise<void> {
    const loaded = await loadExtensions(this.state, urls, () =>
      createExtensionContext(this.state, this.bridge, this.bridge),
    );
    this.extensionDisposables.push(...loaded);
    const finalized = finalizePluginActivity(this.state);
    if (finalized) this.extensionDisposables.push(finalized);
    validateShellState(this.state);
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

export const PLUGIN_ACTIVITY_ID = "plugins";
export const PLUGIN_HOME_SLOT = "plugin.home";

/** Fill `#/plugins/{pluginId}` for owners that only registered extra pages or cards. */
export function finalizePluginActivity(state: ShellState): Disposable | null {
  if (!state.activities.list().some((item) => item.id === PLUGIN_ACTIVITY_ID)) return null;
  const home = state.slots.list().find((item) => item.slot === PLUGIN_HOME_SLOT)?.component as
    | Component
    | undefined;
  if (!home) return null;

  const pages = new Map(state.pages.list().map((page) => [page.id, page]));
  const covered = new Set<string>();
  for (const nav of state.navigation.list()) {
    if (nav.activityId !== PLUGIN_ACTIVITY_ID) continue;
    for (const pluginId of pluginIdsOf(pages.get(nav.pageId))) {
      covered.add(pluginId);
      const label = pluginDisplayName(state, pluginId);
      if (label === pluginId) continue;
      if (nav.label === pluginId) nav.label = label;
      const page = pages.get(nav.pageId);
      if (page && page.title === pluginId) page.title = label;
    }
  }

  const discovered = new Map<string, { label: string; requiredCapability?: string }>();
  const remember = (pluginId: string, label: string, requiredCapability?: string) => {
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
  let order = 1000;
  for (const [pluginId, meta] of discovered) {
    const path = `/plugins/${pluginId}`;
    if (pages.has(pluginId) || [...pages.values()].some((page) => page.path === path)) continue;
    const page: PageRegistration = {
      id: pluginId,
      path,
      title: meta.label,
      pluginId,
      requiredCapability: meta.requiredCapability,
      component: markRaw(
        defineComponent({
          name: "PluginHomePage",
          setup: () => () => h(home, { pluginId }),
        }),
      ),
    };
    scope.own(state.pages.register(page));
    scope.own(
      state.navigation.register({
        id: `${pluginId}.nav`,
        activityId: PLUGIN_ACTIVITY_ID,
        pageId: pluginId,
        label: meta.label,
        order,
        requiredCapability: meta.requiredCapability,
      }),
    );
    order += 1;
    pages.set(pluginId, page);
  }
  return scope;
}

/** Page paths are unique and navigation points at registered pages. Activities are checked
 * against the layout when the shell places navigation. */
export function validateShellState(state: ShellState): void {
  const pages = new Map(state.pages.list().map((item) => [item.id, item]));
  const paths = new Set<string>();
  for (const page of pages.values()) {
    if (!/^\/[a-z0-9][a-z0-9._/-]*$/i.test(page.path)) {
      throw new Error(`invalid page path: ${page.path}`);
    }
    if (paths.has(page.path)) throw new Error(`duplicate page path: ${page.path}`);
    paths.add(page.path);
  }
  for (const item of state.navigation.list()) {
    if (!pages.has(item.pageId)) {
      throw new Error(`navigation ${item.id} references missing page ${item.pageId}`);
    }
  }
}
