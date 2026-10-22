# Bot domain v2 migration

Bot domain v2 makes the Bot contracts platform-neutral, moves owner state onto ServiceHost
application services and unifies plugin naming. Each section lists what changed, what migrates
automatically and what an embedder or external plugin must change. Sections are appended in the
order the changes land on `main`.

## Neutral conversation identity

| Before | After |
| --- | --- |
| `QqConversationRef` | `BotConversationRef` (adds `platform: BotPlatform`) |
| `QqConversationRefError` | `BotConversationRefError` |
| `QQ_CONVERSATION_REF_VERSION` | `BOT_CONVERSATION_REF_VERSION` |
| `BotPlatform::QqBot` serialized as `"qq_bot"` | serialized as `"qqbot"`, equal to `as_str()` |
| — | `BotPlatform::Sandbox` for the simulated adapter |

- `origin_key()` is now `"{platform.origin_prefix()}:v{version}:{encoded}"`. QQ keeps the prefix
  `qq`, so every persisted origin key (ICL, persona bindings, interactions, Agent bindings) is
  byte-identical and no key is rewritten.
- `BotConversationRef` requires `platform` on the wire; there is no default. Peers that send
  conversation refs must add it.
- Bot state database schema v14 (applied automatically on open): every stored JSON document gains
  `"platform": "qqbot"` on conversation refs written before platforms existed, and every
  `"platform": "qq_bot"` tag is rewritten to `"qqbot"`. The migration runs before the sandbox
  history migrations, which decode stored refs. Databases older than v9 are not supported; delete
  `data/bot/state.sqlite3` to start fresh.

## Bot SDK helpers

- `BotContext::{send_message, send_text, upload_media, recall_message}` and their `_with_options`
  forms are removed. Build operations with `prepare_*` and submit them with
  `submit_operation(s)`; only `EffectDrain` surfaces may submit platform operations directly.
- Plugins report infrastructure failures through `mutsuki_bot_sdk::failure` and validate
  registrations through `mutsuki_bot_sdk::manifest::checked`; option-less plugins decode their
  configuration with `mutsuki_bot_sdk::config::decode_empty`.
- The legacy `mutsuki.bot.router.event` configured plugin (which only rejected old configuration)
  is removed; an instance still selecting it fails with "configured plugin factory is not
  registered".

## Adapter contract and QQ protocol split

- New `mutsuki-bot-qq-protocol` crate. Moved out of `mutsuki-bot-protocol`:
  `QQBOT_{RAW_CALL,ACCOUNT_GET,GATEWAY_STATUS,CAPABILITY_GET}_PROTOCOL_ID`,
  `QQBOT_OPENAPI_*_ERROR`, `QqBotCapabilityMatrix`, `QqPermissionRequirement`,
  `QqBotCapabilityGetRequest`, `QqBotAccountGetRequest`, `QqBotGatewayStatusRequest`, and
  `BOT_QQ_REPLY_FORWARD_FOLD_PROTOCOL_ID` (now `QQ_REPLY_FORWARD_FOLD_PROTOCOL_ID`). Protocol id
  and error code strings are unchanged.
- Renamed to platform-neutral types in `mutsuki-bot-protocol`: `QqStreamingStrategy` →
  `BotStreamingStrategy`, `QqUploadConstraints` → `BotUploadConstraints`, `QqRateLimitPolicy` →
  `BotRateLimitPolicy`, `QqMessageSegmentKind` → `BotSegmentKind`.
- `QqBotCapabilityMatrix` now nests the neutral part as `capabilities: BotAdapterCapabilities`
  (`#[serde(flatten)]`), so `capability/get@1` keeps its wire shape; Rust callers read
  `matrix.capabilities.outbound_segments` instead of `matrix.outbound_segments`.
- Adapters declare `BotAdapterDescriptor` as the `mutsuki.bot.adapter@1` manifest extension;
  `qqbot_adapter_manifest_for_accounts` declares the configured QQ account.
- `message/send@1` carries no account; account routing arrived with the sandbox adapter (see
  "Account-routed outbound and the sandbox adapter").

## Host-owned Bot state

- The Bot state database and the sandbox simulator are ServiceHost application services
  (`mutsuki.bot.state`, `mutsuki.bot.sandbox.state`) created on first use through
  `ServiceRuntimeBuilder::ensure_application_service`. The factory-level `OnceLock` slots are gone:
  the sandbox may be selected before or after the QQ adapter, and reconfiguring the sandbox no
  longer fails with "sandbox service already prepared". The simulator keeps its history across
  sandbox reconfigures because it is application-lifetime state.
- Embedders that build factories themselves get the same behavior without extra wiring; nothing
  needs to register the state database explicitly.

## Platform integration crates

- `mutsuki-bot-service-host-integration` no longer assembles QQ or Bilibili. Their factories moved
  to `mutsuki-bot-qq-service-host-integration` (`QqBotConfiguredPlugin`,
  `configured_qq_plugin_catalog`, `QqBotPluginBundle`, `QqAiBotPluginBundle`,
  `QqGatewayEventSource`, `QQBOT_GATEWAY_SOURCE_ID`) and
  `mutsuki-bot-bilibili-service-host-integration` (`BilibiliConfiguredPlugin`,
  `configured_bilibili_plugin_catalog`, `BilibiliPollingEventSource`,
  `BilibiliPollingCredentials`).
- `configured_bot_plugin_catalog*` now return only platform-neutral owners. A product that runs QQ
  or Bilibili merges their catalogs:

  ```rust
  catalog.merge(configured_bot_plugin_catalog_with_agent_and_flow(config.clone(), connections,
      flow_registry.clone(), seed, media.into())?)?;
  catalog.merge(configured_qq_plugin_catalog(media)?)?;
  catalog.merge(configured_bilibili_plugin_catalog(Some(config), Some(flow_registry), media)?)?;
  ```
- The re-exports of `QqGatewayControlHandle`/`QqGatewayHealthHandle`/`QqGatewayHealthSnapshot`/
  `QqInboundObserveHandle` from the core integration crate are removed; import them from
  `mutsuki_plugin_bot_adapter_qqbot::gateway`.

## Product owner registry and reload bindings

- `products/bot` derives every owner-specific list from `owners.rs::product_owners()`. Adding a
  platform owner no longer needs edits in five places.
- Fixed: a hot reload of the `mutsuki.product` document no longer disables
  `mutsuki.std.resource.sqlite` and no longer reloads `mutsuki.std.image.render` without its fonts;
  bootstrap and reload both compile selections through `compile_instance_bindings`.
- The image-render fonts are embedded in the binary and written to `<instance>/fonts` (including
  `OFL.txt`); a moved binary or an external product built from a git dependency works without
  the source tree.
- `TargetedPluginReloadLifecycle::new` takes the instance root as its second argument.

## Unified plugin ids (product config v4)

| Before | After |
| --- | --- |
| `mutsuki.bot.adapter.qqbot` | `mutsuki.plugin.bot.adapter-qqbot` |
| `mutsuki.bot.router.flow` | `mutsuki.plugin.bot.flow` |
| `mutsuki.bot.command` | `mutsuki.plugin.bot.command` |
| `mutsuki.bot.bilibili` | `mutsuki.plugin.bot.bilibili` |
| `mutsuki.bot.bilibili.workshop` | `mutsuki.plugin.bot.bilibili-workshop` |
| `mutsuki.bot.mihuashi` | `mutsuki.plugin.bot.mihuashi` |
| `mutsuki.bot.sandbox` (plugin) | `mutsuki.plugin.bot.sandbox` (`mutsuki.bot.sandbox` stays the host service id); renamed again to `mutsuki.plugin.bot.adapter-sandbox` when the sandbox became an adapter |
| `mutsuki.plugin.bot.conversation.context` | `mutsuki.plugin.bot.conversation-context` |
| `mutsuki.plugin.bot.flow.tool` | `mutsuki.plugin.bot.flow-agent-tool` |

- Runner, protocol, Flow node type and host service ids are unchanged, so stored Flow graphs keep
  validating.
- The single-instance product migrates a v3 config repository once on startup: it copies
  `config.sqlite3` (and `-wal`/`-shm`) to `config.sqlite3.v3.bak`, moves the QQ, Bilibili,
  Workshop and Mihuashi owner documents to their new provider ids and stamps `mutsuki.product` as
  schema/value v4. Documents under the old ids are left in place but unused. To roll back, stop the
  product and restore the `.v3.bak` files over `config.sqlite3*`.
- Hand-written `ServiceConfig` files and `[[plugins.configured]]` entries must use the new ids;
  installed ABI plugin directories named after a plugin id must be renamed.
- Product config versions older than v3 are still rejected; recreate the instance repository.

## Distribution in the single-instance product

- `mutsuki.product` v4 has an optional `distribution` section (`mode`, `deployment`,
  `acceptance`, `fallback`). The production entry now validates it at load, proves the external
  DistributedHost before starting, reports the `distribution` health component and runs the
  sidecar monitor. Absent means `disabled`, so existing instances are unaffected.
- `SingleInstanceProduct` gains `product` (the loaded `mutsuki.product` document);
  `validate_product_distribution` and `prepare_product_distribution` are the document-based
  counterparts of the TOML-file functions.

## Typed console contracts and the platforms surface

- `mutsuki-web-protocol` adds `TypedRpc`/`TypedTopic` and `ts_contract_module`;
  `RpcRegistry::register_typed` checks the capability and decodes params before the handler runs,
  reporting `capability_denied`, `invalid_params` or the handler's own `ExtensionError::Rpc` code.
- New `mutsuki-bot-console-protocol` (pure DTOs) and `mutsuki-bot-web-extension-platforms`
  (`mutsuki.bot.platforms`, namespace `platforms`: `adapters.list`, `accounts.list`,
  `accounts.action`, topic `platforms.changed`). Adapters plug in through
  `mutsuki_bot_management::PlatformConsoleProvider`; QQ and Bilibili providers ship in
  `mutsuki-bot-management`. Providers are resolved on every call, so adapters enabled or
  reloaded after the console started appear without a restart.
- `build_console_host` and `build_console_host_with_agent` are replaced by
  `build_bot_console(config, secrets, control, token, paths, BotConsoleServices { .. })`.
- `attach_management_changed_bridges` takes a `BilibiliResolver` and re-subscribes every owner
  stream after it ends, so `qq.changed`, `bilibili.changed` and `sandbox.changed` keep firing
  across owner reloads; QQ and Bilibili changes also publish `platforms.changed`.

## Account-routed outbound and the sandbox adapter

The sandbox is now a simulated platform adapter instead of an interceptor wrapped around the QQ
runners, and outbound operations route by the bot account they act for.

| Before | After |
| --- | --- |
| `mutsuki.plugin.bot.sandbox` | `mutsuki.plugin.bot.adapter-sandbox` (crate `mutsuki-plugin-bot-adapter-sandbox`; `mutsuki.bot.sandbox` stays the console host service id) |
| Simulated conversations on `BotPlatform::QqBot` with `sandbox:` ids | `BotPlatform::Sandbox`, account `sandbox` |
| `SandboxAwareOpenApiRunner`, `SandboxAwareDeliveryGateway`, `SandboxInterceptHandle`, `is_sandbox_target`, `qq_conversation_from_target` | removed; `sandbox_conversation_from_target` builds sandbox-platform refs |
| `BotAdapterOperations { send_runner_id, recall_runner_id, upload_runner_id }` | `BotAdapterOperations { send, recall, upload }` of `BotAdapterEndpoint { protocol_id, runner_id }` |
| `mutsuki_bot_conversation::qq_conversation_from_event` (always `qqbot`) | `conversation_from_event` (the event's account platform) |
| reply delivery sends `mutsuki.bot.message/send@1` | sends `mutsuki.bot.message/send@2` on `request.conversation.account()` |
| sandbox error `qq.owner_unavailable` for live send | `sandbox.live_unavailable` |
| mention flag `qqbot.mentioned_bot` from the sandbox | neutral `bot.mentioned_bot` (`BOT_EXT_MENTIONED_BOT`); the mention match accepts both |

- New protocols `mutsuki.bot.message/send@2`, `mutsuki.bot.message/recall@2` and
  `mutsuki.bot.media/upload@2` carry `BotAccountRequest { account, request }`. The Host admits one
  runner per protocol, so exactly one runner serves them: the Flow outbound router
  `mutsuki.bot.router.flow.outbound` in `mutsuki.plugin.bot.flow`. It resolves
  `BotAdapterDirectory::route` (published by the Flow LoadPlan observer on
  `BotFlowRegistry::adapters()`) and calls the adapter endpoint by `runner_hint`. QQ endpoints are
  `mutsuki.bot.qqbot.message/send@1`, `mutsuki.bot.qqbot.message/recall@1` and
  `mutsuki.bot.qqbot.media/upload@1`; the sandbox endpoint is
  `mutsuki.bot.sandbox.message/send@1`. A third-party adapter declares its own endpoint protocols
  in its `BotAdapterDescriptor` and must not accept the `@2` ids itself.
- Reply delivery now needs the Flow Router in the plan (it routes through it). Assemblies that
  hand-build Flow must also register `flow_outbound_runner(client, registry)` and publish the
  directory by activating the plan on the registry (`BotFlowRegistry::activate_load_plan`).
- `mutsuki.bot.message/send@1` stays for the persisted QQ sink node `mutsuki.bot.qq.send` and QQ
  management delivery; both only address QQ. It goes away once no stored graph references
  `mutsuki.bot.qq.send`.
- New platform-neutral Flow nodes: sources `mutsuki.bot.message.created|updated|deleted`,
  `mutsuki.bot.reaction.added|removed`, `mutsuki.bot.member.joined|left`,
  `mutsuki.bot.account.connected|disconnected`, `mutsuki.bot.platform.event` (optional
  `platform`/`account` filters) and sinks `mutsuki.bot.send`, `mutsuki.bot.recall`,
  `mutsuki.bot.upload`. The QQ source and sink ids are unchanged, so stored graphs keep
  validating; the QQ sources now accept only `qqbot` events. To run a stored graph in the sandbox,
  swap its message source for `mutsuki.bot.message.created` and its `mutsuki.bot.qq.send` sinks for
  `mutsuki.bot.send` in the Flow editor. The seeded `qq_full_business_flow` (and the committed
  `configs/flow-*.example.json`) already use them for the AI and link chains; the Bilibili push
  chain keeps `mutsuki.bot.qq.send` because notifications carry no account. The seed only applies
  to stores without a flow record, so existing graphs are untouched.
- Sandbox history migrates on load: simulated conversations stored on `qqbot` (any account) move
  to the `sandbox` platform and account, and their conversation and message ids are rewritten to
  the new origin keys. ICL, persona or interaction records keyed by the old `qq:` simulated origin
  keys are not carried over. Live history is unchanged.
- Live mode observes every adapter at Flow ingress (`BotFlowRegistry::set_ingress_observer`)
  instead of the QQ Gateway hook, so it needs the Flow Router; a live send goes through
  `message/send@2` on the real conversation's account and is no longer recorded as a QQ
  management delivery. The QQ adapter no longer publishes `mutsuki.bot.sandbox` itself: the
  sandbox page needs the sandbox adapter selected (the product selects it with the workspace).
- Workspace selections in the single-instance product are derived at startup, so no stored
  config changes. Hand-written `[[plugins.configured]]` entries must rename
  `mutsuki.plugin.bot.sandbox` to `mutsuki.plugin.bot.adapter-sandbox`.
- Fixed: the Flow node runner fixed its callable bindings from the node catalog at construction,
  which ServiceHost does before the LoadPlan observer activates the catalog, so on a fresh start
  every node call failed `undeclared_task_protocol` until a later reload. The node runner and the
  outbound router now build their task-await adapter against `BotFlowRegistry::plan_generation()`.
