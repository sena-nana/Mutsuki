// src/runtime.ts
import {
  WebBridgeClient,
  createRegistry
} from "@mutsuki/web-sdk";
function createShellState() {
  return {
    extensions: [],
    failures: [],
    pages: createRegistry(),
    navigation: createRegistry(),
    slots: createRegistry(),
    commands: createRegistry()
  };
}
async function loadExtensions(state, urls, ctxFactory) {
  const disposables = [];
  for (const item of urls) {
    try {
      const mod = await import(
        /* @vite-ignore */
        item.url
      );
      const result = await mod.default.setup(ctxFactory(state));
      if (result && typeof result.dispose === "function") disposables.push(result);
      state.extensions.push(item.id);
    } catch (error) {
      state.failures.push({
        extensionId: item.id,
        message: error instanceof Error ? error.message : String(error)
      });
    }
  }
  return disposables;
}
var WebShellRuntime = class {
  state = createShellState();
  bridge;
  protocolVersion;
  extensionDisposables = [];
  constructor(options) {
    const { bridgeUrl, protocolVersion = "1.0.0", ...bridgeOptions } = options;
    this.bridge = new WebBridgeClient(bridgeUrl, bridgeOptions);
    this.protocolVersion = protocolVersion;
  }
  get connectionState() {
    return this.bridge.state;
  }
  connect() {
    return this.bridge.connect(this.protocolVersion);
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
    pages: state.pages,
    navigation: state.navigation,
    slots: state.slots,
    commands: state.commands,
    rpc,
    events
  };
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
  loadExtensions
};
