---
name: web-extension-authoring
description: Write or migrate a WebExtension frontend (Vue 3 SFC in an owner crate's web/ package) on @mutsuki/web-sdk v2 — typed RPC contracts, composables, i18n, defineExtension budgets, and fake-bridge tests.
---

# Web Extension Authoring

Use when adding or migrating a console page, overview card or slot contribution. The owner
crate keeps its Rust `WebExtension` (RPC + manifest); the frontend lives in its `web/`
package and is built by `@mutsuki/web-build`.

## Layout

- `crates/<owner>/web/package.json`: private ES module, `catalog:` for vue/vue-router/pinia/
  vitest, `workspace:*` for `@mutsuki/web-sdk`/`@mutsuki/ui`; `tsconfig.json` extends the
  root `tsconfig.web.base.json`. It is a root pnpm workspace member automatically.
- `src/contract.ts` declares the RPCs by hand until the host generates them (B7):
  `defineRpc({ namespace, methods: { list: query<P, R>({ capability }), save: mutation<P, R>() } })`
  plus `topic<T>(name, capability)` for events. Namespace = the Rust registry namespace.
- `src/index.ts` default-exports `defineExtension({ id, setup(ctx) })`. `id` is the owner id
  (`mutsuki.<owner>.<surface>`). Only bare imports of the shared runtime are allowed.

## Data access

- Reads: `useRpc(api.method, params?, { refreshOn: [topic] })` — deduplicated, stale responses
  dropped, refreshed on events/reconnect/invalidation, paused while hidden. Never poll a read
  that has a change topic; use `usePolling` only for sources without one.
- Writes: `useMutation(api.save, { invalidates: [api.list] })`. `delivery_unknown` already
  invalidates; show it as "result unknown, refreshed", never retry a write automatically.
- Errors are `RpcError { code, message, details, retryable }`. Branch on `code`; do not parse
  `message`. Reads time out after 15s, writes after 30s; override per method with `timeoutMs`.
- `useSession().can(capability)` only hides UI; the host enforces every call.
- `useSubscription(topic, handler)`; a denied subscription sets `error` once and is dropped.

## Text and formatting

- All copy goes through `defineMessages({ "zh-CN": { … } })` + `useI18n().t("key", params)`;
  register with `i18n.extend(namespace, messages)`. Locales must define identical keys.
- Use `formatDateTime`/`formatRelativeTime`/`formatDuration`/`formatBytes`, not ad-hoc math.

## Budgets

`defineExtension` fails the extension (never the shell) when setup exceeds 5s, it registers
more than 128 items, or it has more than 16 RPCs in flight; the shell loads at most
`MAX_EXTENSIONS` (64). Raise a budget only with a written reason in `budgets`.

## Tests

Use `@mutsuki/web-sdk/testing`: `createFakeBridge({ handlers: { "ns.method": fn } })`,
`bridge.emit/deny/setState`, `withRuntime(() => useRpc(...), { bridge })` and
`createTestExtensionContext(bridge)` for `setup`. Run under vitest (`happy-dom` for DOM).
Assert behavior (calls made, state shown, errors surfaced), not markup snapshots.
