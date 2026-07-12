---
name: transport-resource
description: Change Python Runner Link transport, stdio JSONL framing, codecs, management messages, resource broker clients, resource plans, descriptor validation, or byte transfer behavior.
---

# Transport And Resource

- Keep transport language-neutral, framed and fail-loud on malformed or unsupported messages.
- Separate stdout protocol frames from stderr diagnostics; never leak secrets into either stream.
- Pass `ResourceRef`/`ValueRef` descriptors across the link and fetch bytes through the broker.
- Validate generation, lifetime, sealing, lease and resource-plan authorization before access.
- Do not add process supervision, environment inheritance or Host lifecycle here.

Test fragmented input, invalid frames, broker failure, stale refs, cancellation and clean EOF.
