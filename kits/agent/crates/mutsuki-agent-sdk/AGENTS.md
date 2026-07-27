# SDK Crate Instructions

- This crate owns Rust ergonomics over existing Mutsuki runtime protocols.
- Do not implement plugin business logic here.
- Do not introduce a second Agent dispatcher; call through `AsyncRunnerContext` and runtime SDK primitives.
- Re-export only stable Agent protocol and helper surfaces from `prelude`.
