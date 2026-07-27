---
name: mutsuki-tauri-approval-security
description: Use when changing approval request events, approval decisions, permission checks, frontend sessions, approval tokens, dev command gating, secrets, or security policy.
---

# Approval And Security Skill

Use this for approval bridge and security-sensitive command paths.

## Boundary

- TauriHost transports approval requests to the UI and returns user decisions to Core/plugin flows.
- Policy decisions may be delegated to Core/plugins/security services; do not hard-code product business rules in Host.
- Secrets never enter frontend payloads or ordinary logs.

## Implementation Rules

- Every approval request needs a nonce/token, operation kind, requester, risk level and trace/correlation context.
- Frontend decisions must reference a live pending request and matching token.
- Dev-only commands default disabled in production profiles.
- External runner environments use allowlists plus injected session values, not inherited full host env.
