---
name: mutsuki-plugin-im-qqbot
description: Postponed Mutsuki QQBot plugin documentation. Use when reviewing the preserved QQBot Gateway/OpenAPI adapter without treating it as a standard plugin.
---

# Mutsuki Postponed QQBot Plugin

This is a postponed business adapter, not part of the first Mutsuki standard
plugin batch. Keep QQBot protocol details inside
`plugins/postponed/mutsuki-plugin-im-qqbot`; do not add QQ concepts to root
`contracts` or `core`, and do not present this package as `mutsuki.std.*`.

## Runtime Surfaces

- Plugin ID: `mutsuki.experimental.im.qqbot`
- Pure runner: `mutsuki.im.qqbot.gateway.normalize`
  - accepts `mutsuki.im.qqbot.gateway.raw`
  - emits `mutsuki.im.qqbot.gateway.ready`, `mutsuki.im.qqbot.gateway.resumed`, `mutsuki.im.qqbot.message.group`,
    `mutsuki.im.qqbot.message.c2c`, `mutsuki.im.qqbot.interaction`, `mutsuki.im.qqbot.lifecycle`,
    `mutsuki.im.qqbot.reaction`, or `mutsuki.im.qqbot.gateway.unknown`
- Effectful runner: `mutsuki.im.qqbot.openapi`
  - accepts `mutsuki.im.qqbot.message.send`
  - accepts `mutsuki.im.qqbot.media.upload`
  - accepts `mutsuki.im.qqbot.message.recall`
  - accepts `mutsuki.im.qqbot.interaction.ack`
  - accepts `mutsuki.im.qqbot.user.share_link`

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
  discrete `mutsuki.im.qqbot.gateway.raw` tasks. Do not model the WebSocket as an
  infinite running task.
- Refresh access tokens before expiry; on HTTP 401 refresh and retry once.
- Retry 429, 5xx, and transient network errors with bounded attempts.
- Reject group messages containing `stream`, `prompt_keyboard`, or
  `action_button`.
- Do not downgrade unsupported media or invalid payloads to text.
- Deduplicate inbound events by `d.id`, then outer `id`, then `s`.
- Redact `clientSecret`, full tokens, and sensitive OpenID lists from errors and
  normal logs.

