# Mutsuki

[简体中文](README.zh-CN.md)

Mutsuki is a domain-neutral meta-framework for **spatiotemporal composition** and
**provenance-aware execution**. It defines how capability-bearing systems are assembled, evolved,
executed, and inspected without binding their business semantics to one application, language, or
deployment topology.

Runtime Core provides a small set of neutral execution laws. Hosts, plugins, domain kits, and
products build their own frameworks above them while retaining explicit ownership boundaries.

## Conceptual model

```text
Capability space        Runtime time           Execution provenance
what can participate  x which world is active -> what happened, why, and from which facts
```

### Task-System architecture

Mutsuki borrows the data/system separation of architectures such as ECS, but it is not an
Entity/Component store. A **Task** is an explicit work fact, a **Runner** supplies behavior, and an
**Executor** supplies the physical execution location. The same capability can therefore run
in-process, across an ABI, in another language, or behind a remote adapter without changing its
business contract. All paths retain the same batch-first execution model.

### Capability-oriented spatial composition

Applications compose capability graphs rather than hard-coded plugin lists. Capability identity is
independent of scope, application projection, implementation generation, and deployment adapter.
Scopes own service dependencies and reversible lifecycle effects; required capabilities fail during
composition, while unsupported optional contributions remain outside the active projection.

### Generational temporal composition

Mutsuki evolves a running system by preparing a new generation instead of mutating the active world
in place. Registries freeze after activation, state and resources carry versions, and execution
attempts are fenced by the generation that authorized them. Reload prepares and validates a
candidate world, switches authority, then drains the old one; stale work cannot silently commit into
the new generation.

### Controlled effects and provenance

Ordinary computation describes outcomes rather than directly rewriting authoritative state or
hiding external side effects. State changes, events, derived work, and effect requests return as
explicit facts through controlled commit boundaries. Task state, resource lineage, ordered events,
and traces then explain which generation accepted work, which attempt ran it, and what it produced.

The default Core is not a permanent event store. Runtime history may be bounded and process-local;
durable audit, portable tasks, checkpoints, and recovery are optional Host/provider capabilities.
External effects retain their real idempotency and compensation limits rather than claiming magical
exactly-once execution.

## Architecture boundaries

This repository is one compatibility baseline, not one package or global feature matrix:

| Path | Ownership |
| --- | --- |
| `crates/mutsuki-runtime-*` | Neutral contracts, wire, Task runtime, Host helpers, and SDK |
| `crates/link/` | Link protocols, transports, discovery, and runtime adapters |
| `hosts/` | Service, CLI, desktop, Web, and optional distributed lifecycle containers |
| `kits/` | AgentKit and Python Runner Kit |
| `plugins/std/` | Domain-neutral resource, effect, workflow, configuration, and observation capabilities |
| `plugins/bot/` | Bot protocols, SDK, routing, platform adapters, and Host integration |
| `products/bot/` | First-party Bot configuration, assembly, and product acceptance |

Core owns neutral execution facts. Hosts own physical execution, lifecycle, supervision, and
configuration. Plugins own domain capabilities. Products select packages without absorbing their
implementations. External products depend only on the packages they need and pin one release
revision.

## Bot reference product

The repository includes a real Bot framework and a runnable first-party product so Mutsuki can be
reviewed end to end rather than only through isolated runtime examples. `plugins/bot` owns reusable
Bot capabilities; `products/bot` only selects configuration and owner catalogs, assembles the
ServiceRuntime, starts the product, and hosts cross-package acceptance. Bot behavior does not enter
Runtime Core.

```bash
cargo run --locked -p mutsuki-bot
```

The first interactive startup creates an executable-adjacent `.mutsuki-bot/` instance and asks for
the Console passphrase. Non-interactive startup supplies it through
`MUTSUKI_SECRET_MUTSUKI_WEB_CONSOLE_TOKEN`. See the
[Bot product guide](products/bot/README.md) for configuration and acceptance boundaries.

To start an external Bot product, install the Cargo subcommand and use its interactive setup to
generate a thin shell pinned to one Mutsuki revision:

```bash
cargo install --locked --path products/bot/crates/mutsuki-create-bot
cargo create-bot
cd my-bot
cargo run
```

For automation, pass all values directly, for example
`cargo create-bot my-bot --revision <40-character-commit>`. The command refuses to overwrite an
existing directory and writes no credentials. The generated project calls the public `mutsuki-bot`
API rather than copying framework or product implementation.

## Development

Rust packages share the root `Cargo.toml` and `Cargo.lock`:

```bash
python3 skills/monorepo-maintenance/scripts/check_workspace.py
cargo metadata --locked --format-version 1
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo test --workspace --all-targets --locked
bash scripts/check-distributed-boundary.sh
```

Python Runner Kit retains its own environment:

```bash
uv run --directory kits/python-runner ruff check src tests
uv run --directory kits/python-runner pyright src tests
uv run --directory kits/python-runner pytest
```

Root performance smoke is `cargo bench-smoke`; see the
[performance model](performance/README.md) for owner baselines and acceptance rules.

## References and acknowledgements

Mutsuki's idea of **spatiotemporal composability** was directly inspired by
[Cordis](https://github.com/cordiverse/cordis) and its reversible plugin model. Cordis provided the
conceptual starting point: temporal composition lets code be loaded together while its effects
remain disposable, and spatial composition makes dependencies explicit and isolatable.

Mutsuki develops that idea independently for a domain-neutral Task runtime, generational execution,
resource facts, and provenance. It does not reuse Cordis APIs or implementation, and does not claim
runtime compatibility with Cordis.

## Further reading

- [Monorepo architecture](docs/architecture/monorepo.md)
- [Runtime architecture](plans/architecture.md)
- [Runtime contracts](plans/contracts.md)
- [Plugin composition](docs/architecture/plugin-capability-composition.md)
- [Release train](docs/release-train.md)
- [First-party Bot decision](docs/decisions/0002-first-party-bot-product.md)

## License

[MIT](LICENSE)
