# Protocol Crate Instructions

- This crate owns Agent wire DTOs and protocol id constants.
- Do not depend on runtime SDK, plugin crates, or provider implementations here.
- Keep DTOs serializable with `serde` and avoid host-specific execution details.
- Any new protocol id must be mirrored in `mutsuki-agent-sdk/src/protocol.rs`.
