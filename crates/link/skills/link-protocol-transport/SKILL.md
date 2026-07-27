---
name: link-protocol-transport
description: Change Mutsuki Link typed envelopes, sessions, channel negotiation, discovery, pairing, authentication, reconnect, Local/TCP/QUIC transports, or transport testkit. Use for code under crates/link that changes protocol or connection behavior.
---

# Link Protocol And Transport

- Keep Link independent of concrete Hosts and business protocols.
- Make negotiated identifiers, capability bits, frame limits and state transitions explicit.
- Bind authentication and authorization to the session and peer identity; reject replay or mismatch.
- Keep control traffic bounded and serviceable during saturated data traffic.
- Preserve one protocol contract across Local, TCP and QUIC; transport differences stay behind adapters.
- Use deterministic transport fakes for behavior tests and real loopback/network smoke only for its stated boundary.

Test negotiation, malformed frames, reconnect, cancellation, backpressure, peer mismatch and shutdown.
