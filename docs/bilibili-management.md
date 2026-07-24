# Bilibili account and subscription management

The Bilibili configured plugin can opt into chat management through its owner-defined
`management` config. Chat and Web Console share `BilibiliManagementService`.

```text
Chat:
  BotEvent -> command parser -> mutsuki.bot.command/handle@1
    -> BilibiliRunner -> BilibiliManagementService -> Host secret / configured-plugin store

Web Console:
  overview /bilibili page -> bilibili.* RPC (runtime.read|write)
    -> BilibiliWebExtension -> same BilibiliManagementService
```

The management contract contains only product fields:

- `enabled` and `command` select the chat command surface.
- `admin_user_ids` authorizes QR login and administrator subscription changes **in chat**.
- `allow_self_binding` enables signature-challenge ownership verification.
- `self_binding_notifications` and `self_binding_outbound_binding` define the subscription created
  after successful verification.
- every subscription has a stable `subscription_id`, `uid`, notification kinds, target, outbound
  binding, `paused`, and optional `owner_user_id`.

Enabling management requires the service to be loaded from a real product config file and requires
Host `security.secret_file`. The product config stores only
`backend = { type = "web_cookie", cookie_secret_key = "..." }`; the ignored secret
file stores the value. Environment-backed secrets are intentionally read-only and cannot be
rotated by QR login.

Full Web Console and chat management require `backend.type = web_cookie` and
`management.enabled = true`. `open_platform` is poll/push only and rejects management.

## Commands

- `/bili login` and `/bili login-status`: administrator QR login. A PNG `ResourceRef` is sent to
  chat; confirmation atomically rotates the Host secret and updates the live credential reader.
- `/bili bind <uid>` and `/bili verify`: self-binding through a short signature challenge. Only a
  verified binding is written into the owner config.
- `/bili unbind`: removes the caller's verified self-binding from owner config.
- `/bili pause [subscription-or-uid]` and `/bili resume [...]`: persist the operational state in
  owner config. The polling EventSource reads the shared current config before every scheduling
  pass.
- `/bili preview [subscription-or-uid]`: fetches and sends the newest dynamic without changing the
  durable poll cursor.
- `/bili list`: lists subscriptions visible to the caller.
- `/bili subscribe <id> <uid> [live,dynamic,video]` and `/bili unsubscribe <id>`: administrator
  management for the current conversation.

## Web Console

When Bilibili management is assembled (`web_cookie` + `management.enabled`),
`BilibiliConsoleBridge` publishes the management service and the embedded console mounts the
`bilibili` WebExtension plus an overview nav entry **B站推送**.

Auth:

- Console holders of `WEB_CONSOLE_AUTH_TOKEN` with `runtime.read` / `runtime.write` act as
  administrators on the Web surface (chat `admin_user_ids` is not re-checked).
- Self-binding RPCs require an explicit `operator_user_id` so `owner_user_id` stays chat-compatible.
- Web `subscribe` requires explicit `target` and `outbound_binding` (chat still uses the current
  conversation target and `self_binding_outbound_binding`).
- Web `preview` returns card JSON only; it does not submit an outbound Bot message.
- Cookie secret values never enter RPC responses, logs, or frontend markup. QR confirmation
  returns a base64 PNG only.

RPC surface (`bilibili` namespace):

- read: `status`, `login.poll`, `subscriptions.list`, `subscriptions.preview`
- write: `login.start`, `credential.clear`, `subscriptions.subscribe`,
  `subscriptions.unsubscribe`, `subscriptions.set_paused`, `binding.start`, `binding.verify`,
  `binding.unbind`

Missing actor identity, authorization, challenge, Host store, secret backend, subscription, or
credential fails with a structured Bilibili runtime error. Cookie values never enter command
payloads, replies, manifests, traces, or ordinary logs.

## Validation levels

- Unit/batch tests use a fake Bilibili transport and real SQLite state to cover secret rotation,
  partial batch failure, signature verification, config persistence, pause, and cursor-free
  preview.
- ServiceHost tests cover atomic shared secret rotation, environment override rejection, and
  owner-only configured-plugin replacement.
- A real-account smoke must use an ignored local product config and secret file. It should verify
  QR confirmation, a signed self-binding, pause/resume, preview, normal polling, and clean Host
  shutdown. Unit or fake coverage must not be reported as a real-account smoke.
