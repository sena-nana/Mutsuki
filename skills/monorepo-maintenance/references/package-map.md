# Mutsuki package map

| Path | Owner boundary |
| --- | --- |
| `crates/mutsuki-runtime-*` | Domain-neutral contracts, wire, kernel, host helpers, SDK, macros and benchmarks |
| `crates/mutsuki-plugin-api`, `crates/mutsuki-plugin-host` | Domain-neutral plugin ABI v2 contracts (FFI-safe types, host gateways) and the dynamic-library loader/lifecycle host |
| `crates/link/` | Link protocols, transports, discovery, pairing and runtime adapters |
| `hosts/cli/` | Service control CLI/TUI client |
| `hosts/service/` | Long-running service lifecycle, config, plugin loading, runner supervision and control |
| `hosts/tauri/` | Desktop lifecycle, Tauri/WebView bridge, resources and frontend client |
| `hosts/web/` | HTTP/WS host, WebExtension runtime, recovery shell and web packages |
| `hosts/distributed/` | Optional distributed sidecar contracts, runtime, control and benchmarks |
| `kits/agent/` | Agent protocols, SDK, plugins, testkit and bundle |
| `kits/python-runner/` | Python Runner SDK, wire mirror, transport and conformance |
| `plugins/bot/` | Bot protocols, SDK, library-surface stores/services (`mutsuki-bot-*`), loadable plugin layer (`mutsuki-plugin-bot-*`), adapters, routing, integration and testkit |
| `plugins/std/` | `protocols/` (`mutsuki-protocol-*`), `plugins/` (`mutsuki-plugin-*` resource/effect/workflow/observe) and supporting `crates/` |
| `products/bot/` | First-party Bot product configuration, runtime assembly, thin external-product scaffold and acceptance |

Dependency direction:

```text
contracts/wire -> runtime core -> host helpers/SDK
plugin abi/host -> ABI plugin loading (domain-neutral, beside the kernel)
link -----------> hosts
runtime SDK ----> standard, agent and bot packages
service host ---> explicit integration packages
all packages ---> first-party and external products
```

The arrows mean “may be consumed by”. Runtime core never depends on concrete hosts, AgentKit,
Bot packages, standard plugins, or products. The plugin ABI contracts and loader stay
domain-neutral and never gain product or host-specific semantics. Link never depends on a
concrete host or business plugin. Agent, Bot, and standard packages remain host-neutral except
explicitly named integration packages.

`products/bot` is the first-party product exception and runs from this Workspace. Business
repositories such as Lilia and Nana remain external consumers and depend on
`https://github.com/sena-nana/Mutsuki.git` at a release tag or fixed commit.
