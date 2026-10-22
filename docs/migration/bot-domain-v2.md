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
