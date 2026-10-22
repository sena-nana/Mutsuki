// @vitest-environment happy-dom
import { afterEach, describe, expect, test, vi } from "vitest";
import { nextTick, ref } from "vue";

import {
  DEFAULT_EXTENSION_BUDGETS,
  RpcError,
  checkMessageKeys,
  createI18n,
  defineExtension,
  defineMessages,
  defineRpc,
  formatBytes,
  formatDateTime,
  formatDuration,
  formatRelativeTime,
  mutation,
  query,
  topic,
  toRpcError,
  useI18n,
  useMutation,
  usePolling,
  useRpc,
  useSession,
  useSubscription,
  type JsonValue,
} from "../src/index.ts";
import {
  createFakeBridge,
  createTestExtensionContext,
  withRuntime,
} from "../src/testing.ts";

const overview = defineRpc({
  namespace: "overview",
  methods: {
    summary: query<{ scope: string } | void, { revision: number }>({ capability: "runtime.read" }),
    restart: mutation<{ id: string }, { ok: boolean }>({ capability: "runtime.write" }),
  },
});
const changed = topic<{ revision: number }>("control.changed", "runtime.read");

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

async function flush(): Promise<void> {
  for (let index = 0; index < 5; index += 1) await Promise.resolve();
  await nextTick();
}

function setHidden(hidden: boolean): void {
  Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden });
  document.dispatchEvent(new Event("visibilitychange"));
}

afterEach(() => {
  setHidden(false);
  vi.useRealTimers();
});

describe("errors", () => {
  test("classify retryable failures and keep delivery_unknown non-retryable", () => {
    expect(new RpcError("disconnected", "x").retryable).toBe(true);
    expect(new RpcError("request_timeout", "x").retryable).toBe(true);
    expect(new RpcError("delivery_unknown", "x").retryable).toBe(false);
    expect(new RpcError("capability_denied", "x").retryable).toBe(false);
    const wrapped = toRpcError(new TypeError("boom"));
    expect(wrapped).toBeInstanceOf(RpcError);
    expect(wrapped.code).toBe("internal");
  });
});

describe("defineRpc", () => {
  test("produces bound method references", () => {
    expect(overview.summary).toMatchObject({
      namespace: "overview",
      method: "summary",
      key: "overview.summary",
      operation: "read",
      capability: "runtime.read",
    });
    expect(overview.restart.operation).toBe("write");
    expect(() => defineRpc({ namespace: "bad space", methods: {} })).toThrow(RpcError);
  });
});

describe("useRpc", () => {
  test("deduplicates identical reads and exposes typed data", async () => {
    const bridge = createFakeBridge();
    const gate = deferred<JsonValue>();
    bridge.handle("overview.summary", () => gate.promise);
    const mounted = withRuntime(
      () => [useRpc(overview.summary), useRpc(overview.summary)] as const,
      { bridge },
    );
    await flush();
    expect(bridge.calls).toHaveLength(1);
    gate.resolve({ revision: 3 });
    await flush();
    const [first, second] = mounted.result;
    expect(first.data.value).toEqual({ revision: 3 });
    expect(second.data.value).toEqual({ revision: 3 });
    expect(first.loading.value).toBe(false);
    mounted.unmount();
  });

  test("drops a response that arrives after a newer request", async () => {
    const bridge = createFakeBridge();
    const pending = new Map<string, ReturnType<typeof deferred<JsonValue>>>();
    bridge.handle("overview.summary", (params) => {
      const gate = deferred<JsonValue>();
      pending.set((params as { scope: string }).scope, gate);
      return gate.promise;
    });
    const scope = ref("old");
    const mounted = withRuntime(() => useRpc(overview.summary, () => ({ scope: scope.value })), {
      bridge,
    });
    await flush();
    scope.value = "new";
    await flush();
    pending.get("new")!.resolve({ revision: 2 });
    await flush();
    pending.get("old")!.resolve({ revision: 1 });
    await flush();
    expect(mounted.result.data.value).toEqual({ revision: 2 });
    mounted.unmount();
  });

  test("refreshes on events, on reconnect, and only once visible again", async () => {
    vi.useFakeTimers();
    const bridge = createFakeBridge();
    let revision = 0;
    bridge.handle("overview.summary", () => ({ revision: ++revision }));
    const mounted = withRuntime(() => useRpc(overview.summary, undefined, { refreshOn: [changed] }), {
      bridge,
    });
    await flush();
    expect(revision).toBe(1);

    bridge.emit("control.changed", { revision: 9 });
    bridge.emit("control.changed", { revision: 10 });
    await vi.advanceTimersByTimeAsync(60);
    expect(revision).toBe(2);

    bridge.setState("reconnecting");
    bridge.setState("open");
    await flush();
    expect(revision).toBe(3);

    setHidden(true);
    bridge.emit("control.changed", { revision: 11 });
    await vi.advanceTimersByTimeAsync(60);
    expect(revision).toBe(3);
    setHidden(false);
    await flush();
    expect(revision).toBe(4);
    expect(mounted.result.data.value).toEqual({ revision: 4 });

    mounted.unmount();
    expect(bridge.subscriptions("control.changed")).toBe(0);
  });

  test("keeps the last data and exposes the structured error", async () => {
    const bridge = createFakeBridge();
    bridge.handle("overview.summary", () => {
      throw new RpcError("capability_denied", "missing runtime.read");
    });
    const mounted = withRuntime(() => useRpc(overview.summary), { bridge });
    await flush();
    expect(mounted.result.error.value?.code).toBe("capability_denied");
    expect(mounted.result.settled.value).toBe(true);
    mounted.unmount();
  });
});

describe("useMutation", () => {
  test("invalidates declared reads on success and on an unknown delivery", async () => {
    const bridge = createFakeBridge();
    let reads = 0;
    bridge.handle("overview.summary", () => ({ revision: ++reads }));
    bridge.handle("overview.restart", () => ({ ok: true }));
    const mounted = withRuntime(
      () => ({
        summary: useRpc(overview.summary),
        restart: useMutation(overview.restart, { invalidates: [overview.summary] }),
      }),
      { bridge },
    );
    await flush();
    expect(reads).toBe(1);
    await expect(mounted.result.restart.mutate({ id: "a" })).resolves.toEqual({ ok: true });
    await flush();
    expect(reads).toBe(2);

    bridge.handle("overview.restart", () => {
      throw new RpcError("delivery_unknown", "write may have been delivered");
    });
    await expect(mounted.result.restart.mutate({ id: "a" })).rejects.toMatchObject({
      code: "delivery_unknown",
    });
    await flush();
    expect(reads).toBe(3);
    expect(mounted.result.restart.error.value?.code).toBe("delivery_unknown");
    expect(mounted.result.restart.pending.value).toBe(false);
    mounted.unmount();
  });
});

describe("useSubscription", () => {
  test("delivers payloads and reports a denied subscription", async () => {
    const bridge = createFakeBridge();
    bridge.deny("secret.changed");
    const seen: number[] = [];
    const mounted = withRuntime(
      () => ({
        ok: useSubscription(changed, (payload) => seen.push(payload.revision)),
        denied: useSubscription("secret.changed", () => undefined),
      }),
      { bridge },
    );
    bridge.emit("control.changed", { revision: 5 });
    await flush();
    expect(seen).toEqual([5]);
    expect(mounted.result.ok.error.value).toBeNull();
    expect(mounted.result.denied.error.value?.code).toBe("capability_denied");
    mounted.unmount();
    expect(bridge.subscriptions("control.changed")).toBe(0);
  });
});

describe("usePolling", () => {
  test("never overlaps runs and pauses while hidden", async () => {
    vi.useFakeTimers();
    let runs = 0;
    const gate = deferred<void>();
    const mounted = withRuntime(() =>
      usePolling(async () => {
        runs += 1;
        if (runs === 1) await gate.promise;
      }, 1_000),
    );
    await vi.advanceTimersByTimeAsync(5_000);
    expect(runs).toBe(1);
    gate.resolve();
    await vi.advanceTimersByTimeAsync(1_000);
    expect(runs).toBe(2);
    setHidden(true);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(runs).toBe(2);
    setHidden(false);
    await flush();
    expect(runs).toBe(3);
    mounted.unmount();
    await vi.advanceTimersByTimeAsync(5_000);
    expect(runs).toBe(3);
  });
});

describe("useSession", () => {
  test("gates capabilities and reports an unauthorized connection", async () => {
    const bridge = createFakeBridge();
    const login = vi.fn(async (token: string) => token === "right");
    const mounted = withRuntime(() => useSession(), {
      bridge,
      session: {
        capabilities: () => new Set(["runtime.read"]),
        login,
        logout: async () => undefined,
      },
    });
    const session = mounted.result;
    expect(session.can("runtime.read")).toBe(true);
    expect(session.can("runtime.write")).toBe(false);
    expect(session.can(undefined)).toBe(true);
    expect(session.unauthorized.value).toBe(false);
    bridge.setState("unauthorized");
    await nextTick();
    expect(session.unauthorized.value).toBe(true);
    await expect(session.login("wrong")).resolves.toBe(false);
    mounted.unmount();
  });
});

describe("i18n", () => {
  const messages = defineMessages({
    "zh-CN": { overview: { title: "概览", uptime: "已运行 {duration}" } },
  });

  test("translates with interpolation and echoes missing keys", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    const i18n = createI18n({ messages });
    expect(i18n.t("overview.title")).toBe("概览");
    expect(i18n.t("overview.uptime", { duration: "3 分" })).toBe("已运行 3 分");
    expect(i18n.t("overview.missing" as never)).toBe("overview.missing");
    expect(warn).toHaveBeenCalledTimes(1);
    warn.mockRestore();
  });

  test("namespaces extensions and rejects duplicate or inconsistent keys", () => {
    const i18n = createI18n().extend("qq", messages);
    expect(i18n.t("qq.overview.title")).toBe("概览");
    expect(() => i18n.extend("qq", messages)).toThrow(/duplicate/);
    expect(checkMessageKeys({ "zh-CN": { a: "1", b: { c: "2" } }, "en-US": { a: "1" } })).toEqual([
      "en-US:b.c",
    ]);
  });

  test("useI18n resolves the provided catalog", () => {
    const i18n = createI18n({ messages });
    const mounted = withRuntime(() => useI18n<(typeof messages)["zh-CN"]>(), { i18n });
    expect(mounted.result.t("overview.title")).toBe("概览");
    mounted.unmount();
  });
});

describe("formatters", () => {
  test("format durations, bytes and times in zh-CN", () => {
    expect(formatDuration(42_000)).toBe("42 秒");
    expect(formatDuration(65_000)).toBe("1 分 5 秒");
    expect(formatDuration(3_900_000)).toBe("1 小时 5 分");
    expect(formatDuration(90_000_000)).toBe("1 天 1 小时");
    expect(formatDuration(null)).toBe("—");
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1536)).toBe("1.5 KiB");
    expect(formatBytes(5 * 1024 ** 3)).toBe("5 GiB");
    const now = Date.UTC(2026, 9, 22, 12, 0, 0);
    expect(formatRelativeTime(now - 3 * 60_000, now)).toBe("3分钟前");
    expect(formatRelativeTime(now + 2 * 3_600_000, now)).toBe("2小时后");
    expect(formatRelativeTime("not a date", now)).toBe("—");
    expect(formatDateTime(new Date(2026, 9, 22, 13, 5, 9))).toBe("2026/10/22 13:05:09");
  });
});

describe("defineExtension", () => {
  test("rejects invalid ids and enforces the registration budget", async () => {
    expect(() => defineExtension({ id: "Bad Id", setup() {} })).toThrow(RpcError);
    const extension = defineExtension({
      id: "mutsuki.test",
      budgets: { maxRegistrations: 2 },
      setup(ctx) {
        expect(ctx.extensionId).toBe("mutsuki.test");
        ctx.slots.register({
          id: "card",
          slot: "overview.cards",
          component: { mount() {} },
        });
        ctx.commands.register({ id: "one", title: "one", run() {} });
        ctx.commands.register({ id: "two", title: "two", run() {} });
      },
    });
    await expect(extension.setup(createTestExtensionContext())).rejects.toMatchObject({
      code: "budget_exceeded",
    });
  });

  test("bounds concurrent RPCs and the setup deadline", async () => {
    const bridge = createFakeBridge();
    const gate = deferred<JsonValue>();
    bridge.handle("slow.wait", () => gate.promise);
    let rpc!: ReturnType<typeof createTestExtensionContext>["rpc"];
    await defineExtension({
      id: "mutsuki.rpc",
      budgets: { maxConcurrentRpc: 1 },
      setup(ctx) {
        rpc = ctx.rpc;
      },
    }).setup(createTestExtensionContext(bridge));
    const first = rpc.read("slow", "wait");
    await expect(rpc.read("slow", "wait")).rejects.toMatchObject({ code: "pending_limit" });
    gate.resolve(null);
    await first;

    vi.useFakeTimers();
    const hanging = defineExtension({
      id: "mutsuki.hang",
      setup: () => new Promise(() => undefined),
    }).setup(createTestExtensionContext());
    const assertion = expect(hanging).rejects.toMatchObject({ code: "extension_setup_timeout" });
    await vi.advanceTimersByTimeAsync(DEFAULT_EXTENSION_BUDGETS.setupTimeoutMs);
    await assertion;
  });
});
