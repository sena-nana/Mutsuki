import assert from "node:assert/strict";
import test from "node:test";

import {
  createExtensionContext,
  createShellState,
  createWebUiThemeController,
  finalizePluginActivity,
  groupNavigationItems,
  loadExtensions,
  validateShellState,
} from "../dist/runtime.js";

function registry(label, disposed) {
  return {
    register() {
      return { dispose: () => disposed.push(label) };
    },
    list() {
      return [];
    },
  };
}

function context(disposed) {
  return {
    activities: registry("activity", disposed),
    pages: registry("page", disposed),
    navigation: registry("navigation", disposed),
    slots: registry("slot", disposed),
    commands: registry("command", disposed),
    rpc: {},
    events: {
      subscribe() {
        return { dispose: () => disposed.push("event") };
      },
    },
  };
}

function extensionUrl(key) {
  return `data:text/javascript,export default {setup(ctx){return globalThis.${key}(ctx)}}`;
}

test("extension scope rolls back setup failure and disposes successful effects in reverse", async () => {
  const disposed = [];
  const state = createShellState();
  globalThis.__mutsukiSuccessfulExtension = (ctx) => {
    ctx.pages.register({});
    ctx.events.subscribe("topic", () => undefined);
    return { dispose: () => disposed.push("setup") };
  };

  const scopes = await loadExtensions(
    state,
    [{ id: "successful", url: extensionUrl("__mutsukiSuccessfulExtension") }],
    () => context(disposed),
  );
  assert.deepEqual(state.extensions, ["successful"]);
  assert.equal(scopes.length, 1);
  scopes[0].dispose();
  scopes[0].dispose();
  assert.deepEqual(disposed, ["setup", "event", "page"]);

  globalThis.__mutsukiFailingExtension = (ctx) => {
    ctx.navigation.register({});
    throw new Error("setup failed");
  };
  await loadExtensions(
    state,
    [{ id: "failing", url: extensionUrl("__mutsukiFailingExtension") }],
    () => context(disposed),
  );
  assert.deepEqual(disposed, ["setup", "event", "page", "navigation"]);
  assert.equal(state.failures.at(-1).extensionId, "failing");
  assert.match(state.failures.at(-1).message, /setup failed/);

  delete globalThis.__mutsukiSuccessfulExtension;
  delete globalThis.__mutsukiFailingExtension;
});

test("shell contract rejects dangling navigation and duplicate page paths", () => {
  const state = createShellState();
  state.activities.register({ id: "home", label: "Home", icon: "home" });
  state.pages.register({
    id: "one",
    path: "/one",
    title: "One",
    component: { mount() {} },
  });
  state.navigation.register({
    id: "one.nav",
    activityId: "home",
    pageId: "missing",
    label: "Missing",
  });
  assert.throws(() => validateShellState(state), /missing page/);

  state.navigation.clear();
  state.pages.register({
    id: "two",
    path: "/one",
    title: "Two",
    component: { mount() {} },
  });
  assert.throws(() => validateShellState(state), /duplicate page path/);
});

test("navigation grouping preserves ungrouped entries and contiguous sections", () => {
  const items = [
    { id: "primary", label: "Mutsuki" },
    { id: "qq", label: "QQ", group: "插件" },
    { id: "agent", label: "Agent", group: "插件" },
  ];
  assert.deepEqual(
    groupNavigationItems(items).map((section) => ({
      group: section.group,
      ids: section.items.map((item) => item.id),
    })),
    [
      { group: undefined, ids: ["primary"] },
      { group: "插件", ids: ["qq", "agent"] },
    ],
  );
});

test("theme controller persists preference, follows system, and releases listener", () => {
  const values = new Map();
  const storage = {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
  };
  let mediaListener;
  const media = {
    matches: false,
    addEventListener: (_event, listener) => { mediaListener = listener; },
    removeEventListener: (_event, listener) => {
      assert.equal(listener, mediaListener);
      mediaListener = undefined;
    },
  };
  const root = { dataset: {}, style: {} };
  globalThis.document = { documentElement: root };

  const controller = createWebUiThemeController({ storage, media, storageKey: "test.theme" });
  assert.equal(controller.preference, "system");
  assert.equal(root.style.colorScheme, "dark");
  media.matches = true;
  mediaListener();
  assert.equal(root.style.colorScheme, "light");
  controller.setPreference("dark");
  assert.equal(values.get("test.theme"), "dark");
  media.matches = true;
  mediaListener();
  assert.equal(root.style.colorScheme, "dark");
  controller.dispose();
  assert.equal(mediaListener, undefined);
  delete globalThis.document;
});

test("slot contributions remain available after all extensions load", async () => {
  const state = createShellState();
  globalThis.__mutsukiSlotHost = () => undefined;
  globalThis.__mutsukiSlotGuest = (ctx) => {
    ctx.slots.register({
      id: "guest.card",
      slot: "overview.cards",
      component: { mount() {} },
    });
  };
  await loadExtensions(
    state,
    [
      { id: "host", url: extensionUrl("__mutsukiSlotHost") },
      { id: "guest", url: extensionUrl("__mutsukiSlotGuest") },
    ],
    () => createExtensionContext(state, {}, { subscribe() { return { dispose() {} }; } }),
  );
  assert.deepEqual(
    state.slots.list().map((item) => item.id),
    ["guest.card"],
  );
  delete globalThis.__mutsukiSlotHost;
  delete globalThis.__mutsukiSlotGuest;
});

test("activity registration retains the first descriptor and is refcounted", () => {
  const state = createShellState();
  const first = state.activities.register({ id: "bot", label: "Bot", icon: "bot" });
  const second = state.activities.register({ id: "bot", label: "Other", icon: "x" });
  assert.equal(state.activities.list().length, 1);
  assert.equal(state.activities.list()[0].label, "Bot");
  second.dispose();
  assert.equal(state.activities.list().length, 1);
  first.dispose();
  assert.equal(state.activities.list().length, 0);
});

test("finalizePluginActivity adds hub pages for extra plugin-owned pages", () => {
  const state = createShellState();
  state.activities.register({ id: "plugins", label: "插件", icon: "config" });
  state.activities.register({ id: "bot", label: "Bot", icon: "bot" });
  state.slots.register({
    id: "plugin.home",
    slot: "plugin.home",
    component: {
      mount() {
        return { dispose() {} };
      },
    },
  });
  state.pages.register({
    id: "bilibili.page",
    path: "/bilibili",
    title: "B站推送",
    pluginId: "mutsuki.bot.bilibili",
    component: { mount() {} },
  });
  state.navigation.register({
    id: "bilibili.nav",
    activityId: "bot",
    pageId: "bilibili.page",
    label: "B站推送",
  });

  const disposable = finalizePluginActivity(state);
  assert.ok(state.pages.list().some((page) => page.id === "mutsuki.bot.bilibili"));
  assert.ok(
    state.navigation.list().some(
      (item) => item.activityId === "plugins" && item.pageId === "mutsuki.bot.bilibili",
    ),
  );
  validateShellState(state);
  disposable.dispose();
  assert.equal(state.pages.list().some((page) => page.id === "mutsuki.bot.bilibili"), false);
});

function configExtensionUrl() {
  return new URL(
    "../../../../../plugins/std/plugins/mutsuki-plugin-config-web/assets/index.js",
    import.meta.url,
  ).href;
}

async function loadConfigExtension(rpc) {
  const state = createShellState();
  state.activities.register({ id: "plugins", label: "插件", icon: "config" });
  await loadExtensions(
    state,
    [{ id: "config", url: configExtensionUrl() }],
    () => createExtensionContext(state, rpc, { subscribe() { return { dispose() {} }; } }),
  );
  assert.deepEqual(state.failures, []);
  return state;
}

function pluginRpc({ navigation, plugins = [], pluginListError } = {}) {
  return {
    async call(namespace, method, params = {}) {
      if (namespace === "config" && method === "navigation.list") return navigation;
      if (namespace === "config" && method === "schema.get") {
        return { title: { default: params.provider_id } };
      }
      if (namespace === "control" && method === "plugin_list") {
        if (pluginListError) throw pluginListError;
        return { plugins };
      }
      throw new Error(`unexpected call ${namespace}/${method}`);
    },
    read(namespace, method, params) {
      return this.call(namespace, method, params);
    },
  };
}

test("config extension lists loaded plugins that are not config providers", async () => {
  const state = await loadConfigExtension(pluginRpc({
    navigation: [{ label: "接入", items: [{ provider_id: "mutsuki.bot.adapter.qqbot", label: "QQ 登录" }] }],
    plugins: [
      { plugin_id: "mutsuki.bot.adapter.qqbot", configured: true, active_deployment: "builtin" },
      { plugin_id: "mutsuki.bot.mihuashi", configured: true, active_deployment: "builtin" },
      { plugin_id: "mutsuki.plugin.resource.memory", configured: false, active_deployment: "builtin" },
      { plugin_id: "mutsuki.catalog.only", configured: false, active_deployment: null },
    ],
  }));
  const nav = Object.fromEntries(state.navigation.list().map((item) => [item.pageId, item]));
  assert.equal(nav["mutsuki.bot.adapter.qqbot"].label, "QQ 登录");
  assert.equal(nav["mutsuki.bot.adapter.qqbot"].group, "接入");
  assert.equal(nav["mutsuki.bot.mihuashi"].group, "已加载");
  assert.equal(nav["mutsuki.plugin.resource.memory"].group, "已加载");
  assert.equal(nav["mutsuki.catalog.only"], undefined);
  validateShellState(state);
});

test("config extension keeps provider navigation when plugin_list fails", async () => {
  const state = await loadConfigExtension(pluginRpc({
    navigation: [{ items: [{ provider_id: "product", label: "工作区" }] }],
    pluginListError: new Error("control unavailable"),
  }));
  assert.ok(state.navigation.list().some((item) => item.pageId === "product" && item.label === "工作区"));
  assert.equal(state.navigation.list().some((item) => item.group === "已加载"), false);
  validateShellState(state);
});
