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
  (`mutsuki.<owner>.<surface>`, same as the Rust `PLUGIN_ID`). Only bare imports of the shared
  runtime are allowed.
- Build with `mutsuki-web-build build src/index.ts browser --no-manifest`; commit `browser/`.
  The Rust crate `include_str!`s `web/browser/index.js` (and `index.css` if any) and derives the
  manifest itself, so there is one source of truth for capabilities.
- The RPC namespace is `WebExtension::namespace()` in Rust (defaults to the id). A qualified id
  keeps a short namespace (`mutsuki.web.diagnostics` → `control`); two extensions cannot share
  one namespace. Pages registered by a migrated extension keep their old page ids so saved
  routes keep working.

## Shell contract

- Activities belong to the product layout (`ConsoleLayout`, e.g. `console_layout()` in
  `products/bot`); an extension registers pages and navigation into existing activity ids and
  never calls `ctx.activities.register` (unknown ids land in the shell's failures panel).
- Register a Vue component as `PageRegistration.component`; plain `{ mount }` objects still work
  through `legacyMount` until W11. Each page renders inside an `ExtensionBoundary`.
- Header actions teleport into `#mk-page-actions`; the shell owns title, skip link, landmarks and
  the connection live region.

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

## UI

- Build pages from `@mutsuki/ui` `Mk*` components; style only with `--mk-*` tokens (hex colors
  fail the build). `MkAsync` renders a `useRpc` result's loading/error/empty/data states.
- Confirm destructive actions with `useConfirm({ danger: true, requireText })`, ask with
  `usePrompt`, report with `useToast`; never `window.confirm/prompt/alert` or `innerHTML`.
- Icon-only controls need a label (`MkIconButton label`); inputs live in `MkField` or carry
  `label`; secrets use `MkSecretInput` and are never read back.

## Text and formatting

- All copy goes through `defineMessages({ "zh-CN": { … } })`; components read it with
  `useMessages(messages).t("key", params)` (follows the shell locale, keys checked at compile
  time). Locales must define identical keys.
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
