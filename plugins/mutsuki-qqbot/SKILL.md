---
name: mutsuki-qqbot
description: MutsukiCore runtime plugin documentation for QQBot Gateway/OpenAPI integration. Use when wiring, reviewing, or extending the Rust QQBot plugin surfaces, task payloads, host-injected credentials, Gateway pump, media upload, or OpenAPI effect runner behavior.
---

# MutsukiCore QQBot Plugin

This is a MutsukiCore plugin, not a Codex plugin. Keep QQBot protocol details inside
`plugins/mutsuki-qqbot`; do not add QQ concepts to root `contracts` or `core`.

## Runtime Surfaces

- Plugin ID: `mutsuki.qqbot`
- Pure runner: `mutsuki.qqbot.gateway.normalize`
  - accepts `raw.input.qqbot.gateway`
  - emits `qqbot.gateway.ready`, `qqbot.gateway.resumed`, `qqbot.message.group`,
    `qqbot.message.c2c`, `qqbot.interaction`, `qqbot.lifecycle`,
    `qqbot.reaction`, or `qqbot.gateway.unknown`
- Effectful runner: `effect.qqbot.openapi`
  - accepts `effect.qqbot.message.send`
  - accepts `effect.qqbot.media.upload`
  - accepts `effect.qqbot.message.recall`
  - accepts `effect.qqbot.interaction.ack`
  - accepts `effect.qqbot.user.share_link`

## Host Wiring

Inject `QqBotConfig`, `QqHttpClient`, `QqMediaProvider`, and `QqIdSource` from the
host. Never put `client_secret`, full access tokens, or runtime HTTP handles in
`Task.payload`, root contracts, or logs.

Use `qqbot_manifest()` to register plugin surfaces and `qqbot_runners(...)` to
materialize runner objects for a runtime generation.

## Task Payloads

All effect payloads are JSON objects.

- Message send:
  - `scene`: `group` or `c2c`
  - `target_openid`: group OpenID or user OpenID
  - `body`: QQ message body fields such as `msg_type`, `content`, `markdown`,
    `keyboard`, `media`, `image`, `ark`, `embed`, `message_reference`,
    `event_id`, `msg_id`, `is_wakeup`, `stream`, `prompt_keyboard`,
    `action_button`, and optional `timestamp`
- Media upload:
  - `scene`, `target_openid`, `file_type`
  - one upload source: `url`, `file_data`, `resource_ref`, or `upload_id`
  - optional `srv_send_msg`, `file_name`, `file_size`, `md5`, `sha1`, `md5_10m`
- Recall:
  - `scene`, `target_openid`, `message_id`
- Interaction ACK:
  - `interaction_id`, `code`
- User share link:
  - optional `callback_data`

## Operational Rules

- QQ credentials come from host injection only.
- Gateway is a host helper (`QqGatewayPump`) that turns dispatch frames into
  discrete `raw.input.qqbot.gateway` tasks. Do not model the WebSocket as an
  infinite running task.
- Refresh access tokens before expiry; on HTTP 401 refresh and retry once.
- Retry 429, 5xx, and transient network errors with bounded attempts.
- Reject group messages containing `stream`, `prompt_keyboard`, or
  `action_button`.
- Do not downgrade unsupported media or invalid payloads to text.
- Deduplicate inbound events by `d.id`, then outer `id`, then `s`.
- Redact `clientSecret`, full tokens, and sensitive OpenID lists from errors and
  normal logs.

