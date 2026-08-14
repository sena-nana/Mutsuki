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
      const mod = await import(
        /* @vite-ignore */
        item.url
      );
      const result = await mod.default.setup(ownedExtensionContext(ctxFactory(state), scope));
      if (result && typeof result.dispose === "function") scope.own(result);
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
