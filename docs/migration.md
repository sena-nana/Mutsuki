# QQBot Migration

The old NanoBot postponed plugin `plugins/postponed/mutsuki-plugin-im-qqbot` was split into this repository.

Mapping:

- Old gateway pump -> `mutsuki-plugin-bot-adapter-qqbot::gateway`
- Old gateway normalizer -> `adapter::event_map`
- Old OpenAPI runner -> `api` plus `tasks`
- Old payload validation -> `api::message` and `tasks`
- Old media provider -> `api::media`
- Old `mutsuki.im.qqbot.*` IDs -> standard `mutsuki.bot.*` and QQBot-specific `mutsuki.bot.qqbot.*`

The old location should stay deleted after this repository owns the adapter.
