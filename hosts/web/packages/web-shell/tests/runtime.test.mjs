import assert from "node:assert/strict";
import test from "node:test";

import {
  createShellState,
  createWebUiThemeController,
  groupNavigationItems,
  loadExtensions,
  validateShellState,
} from "../dist/runtime.js";

function registry(label, disposed) {
  return {
    register() {
      return { dispose: () => disposed.push(label) };
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
