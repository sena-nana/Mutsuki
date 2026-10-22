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
- Still pending: `message/send@1` carries no account. Account routing via
  `BotAdapterDirectory` and `runner_hint` arrives with the simulated sandbox adapter.

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
| `mutsuki.bot.sandbox` (plugin) | `mutsuki.plugin.bot.sandbox` (`mutsuki.bot.sandbox` stays the host service id) |
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
