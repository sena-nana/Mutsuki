import assert from "node:assert/strict";
import test from "node:test";

import { createShellState, loadExtensions } from "../dist/runtime.js";

function registry(label, disposed) {
  return {
    register() {
      return { dispose: () => disposed.push(label) };
    },
  };
}

function context(disposed) {
  return {
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
