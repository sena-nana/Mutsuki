# Cross-package integration tests

Cross-package tests remain beside the product or Host that owns the acceptance boundary, so they can
use the same fixtures and are selected by the unified root Workspace:

- `templates/bot/crates/mutsuki-bot/tests`: Core + ServiceHost + StdPlugins + BotPlugins + AgentKit
  assembly, configuration failure semantics, health and shutdown.
- `hosts/service/crates/mutsuki-service-runtime/tests`: ServiceHost lifecycle and plugin loading.
- `hosts/distributed/crates/mutsuki-distributed-runtime/tests`: Link-backed placement, recovery and
  failover.
- `hosts/tauri/crates/mutsuki-tauri-host/tests`: embedded Host and bridge behavior.
- `hosts/web/crates/mutsuki-web-host/tests`: HTTP/WebSocket bridge and recovery shell behavior.
- `kits/python-runner/tests`: cross-language Runner Wire conformance.

`cargo test --workspace --all-targets --locked` is the Rust integration gate. Python and frontend
gates are defined in `.github/workflows/ci.yml`. Tests must assert observable behavior; this directory
is an index, not a place for duplicate string- or log-matching tests.
