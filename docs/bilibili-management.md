# Bilibili account and subscription management

The Bilibili configured plugin can opt into chat management through its owner-defined
`management` config. The runtime path remains:

```text
BotEvent -> command parser -> mutsuki.bot.command/handle@1
  -> Bilibili batch runner -> Bot message task / Host persistence boundary
```

The management contract contains only product fields:

- `enabled` and `command` select the chat command surface.
- `admin_user_ids` authorizes QR login and administrator subscription changes.
- `allow_self_binding` enables signature-challenge ownership verification.
- `self_binding_notifications` and `self_binding_outbound_binding` define the subscription created
  after successful verification.
- every subscription has a stable `subscription_id`, `uid`, notification kinds, target, outbound
  binding, `paused`, and optional `owner_user_id`.

Enabling management requires the service to be loaded from a real product config file and requires
Host `security.secret_file`. The product config stores only `cookie_secret_key`; the ignored secret
file stores the value. Environment-backed secrets are intentionally read-only and cannot be
rotated by QR login.

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
