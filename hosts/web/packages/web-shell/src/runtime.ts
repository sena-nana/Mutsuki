import {
  WebBridgeClient,
  createRegistry,
  type BridgeConnectionState,
  type BridgeHelloAck,
  type Disposable,
  type ExtensionContext,
  type WebBridgeClientOptions,
  type WebExtension,
} from "@mutsuki/web-sdk";

export interface ShellState {
  extensions: string[];
  failures: Array<{ extensionId: string; message: string }>;
  pages: ReturnType<
    typeof createRegistry<{ id: string; path: string; title: string; component: unknown }>
  >;
  navigation: ReturnType<
    typeof createRegistry<{ id: string; label: string; path: string }>
  >;
  slots: ReturnType<typeof createRegistry<{ id: string; slot: string; component: unknown }>>;
  commands: ReturnType<
    typeof createRegistry<{ id: string; title: string; run: () => void | Promise<void> }>
  >;
}

export function createShellState(): ShellState {
  return {
    extensions: [],
    failures: [],
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
    try {
      const mod = (await import(/* @vite-ignore */ item.url)) as {
        default: WebExtension;
      };
      const result = await mod.default.setup(ctxFactory(state));
      if (result && typeof result.dispose === "function") disposables.push(result);
      state.extensions.push(item.id);
    } catch (error) {
      state.failures.push({
        extensionId: item.id,
        message: error instanceof Error ? error.message : String(error),
      });
    }
  }
  return disposables;
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

  constructor(options: WebShellRuntimeOptions) {
    const { bridgeUrl, protocolVersion = "1.0.0", ...bridgeOptions } = options;
    this.bridge = new WebBridgeClient(bridgeUrl, bridgeOptions);
    this.protocolVersion = protocolVersion;
  }

  get connectionState(): BridgeConnectionState {
    return this.bridge.state;
  }

  connect(): Promise<BridgeHelloAck> {
    return this.bridge.connect(this.protocolVersion);
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
    pages: state.pages,
    navigation: state.navigation,
    slots: state.slots,
    commands: state.commands,
    rpc,
    events,
  };
}

export const SHARED_IMPORT_MAP = {
  vue: "/shared/vue.js",
  "vue-router": "/shared/vue-router.js",
  pinia: "/shared/pinia.js",
  "@mutsuki/web-sdk": "/shared/web-sdk.js",
  "@mutsuki/ui": "/shared/ui.js",
} as const;
