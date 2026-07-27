# Mutsuki

Mutsuki is a domain-neutral, batch-first runtime framework maintained as one monorepo with
independently consumable packages. The repository contains the Rust runtime and Link layers,
product Hosts, Agent and Python kits, Bot and standard plugins, canonical templates, integration
tests, performance models, and one release compatibility baseline.

Repository consolidation does not create an all-in-one crate. Products depend only on the packages
they need:

```toml
[dependencies]
mutsuki-runtime-core = { git = "https://github.com/sena-nana/Mutsuki.git", tag = "v0.1.0" }
mutsuki-link = { git = "https://github.com/sena-nana/Mutsuki.git", tag = "v0.1.0" }
mutsuki-tauri-host = { git = "https://github.com/sena-nana/Mutsuki.git", tag = "v0.1.0" }
```

## Package groups

| Path | Packages |
| --- | --- |
| `crates/mutsuki-runtime-*` | Contracts, wire, CoreRuntime, Host helpers, Rust SDK and benchmarks |
| `crates/link/` | Link protocol, discovery, pairing, Local/TCP/QUIC and runtime adapters |
| `hosts/` | CLI, service, Tauri, Web and optional distributed Hosts |
| `kits/` | AgentKit and Python Runner Kit |
| `plugins/` | Bot packages and domain-neutral standard protocols/plugins |
| `templates/bot/` | Canonical Bot template source and export tooling |

The full ownership and dependency map is in
[Monorepo architecture](docs/architecture/monorepo.md). Public runtime contracts remain
batch-first and preserve TaskPool, TaskLease, TaskHandle, ResourceRef, LoadPlan and structured
failure boundaries.

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

Python Runner Kit:

```bash
cd kits/python-runner
uv run ruff check src tests
uv run pyright src tests
uv run pytest
```

Web and Tauri frontend packages retain their package-level scripts and lockfiles under
`hosts/web` and `hosts/tauri`. Run their typecheck/build commands when those surfaces change.

## Performance

Each owner boundary retains its workload, raw samples and anomaly analysis. Root Core smoke:

```bash
cargo bench-smoke
```

Owner smoke entrypoints:

```bash
python3 crates/link/scripts/run-performance-model.py \
  --mode smoke --output target/mutsuki-benchmarks/link-smoke.json
python3 hosts/service/crates/mutsuki-service-benchmarks/scripts/run-reference.py \
  --mode smoke --warmup 0 --samples 1 \
  --output target/mutsuki-benchmarks/service-host-smoke.json
python3 hosts/tauri/crates/mutsuki-tauri-benchmarks/scripts/run-reference.py \
  --mode smoke --warmup 0 --samples 1 \
  --output target/mutsuki-benchmarks/tauri-smoke.json
python3 hosts/distributed/scripts/run-performance-model.py \
  --mode smoke --service-binary target/release/mutsuki-benchmark-service \
  --output target/mutsuki-benchmarks/distributed-smoke.json
python3 kits/agent/scripts/run-performance-model.py \
  --mode smoke --output target/mutsuki-benchmarks/agent-smoke.json
python3 plugins/bot/scripts/run-performance-model.py \
  --mode smoke --output target/mutsuki-benchmarks/bot-smoke.json
python3 plugins/std/scripts/run-performance-model.py \
  --mode smoke --output target/mutsuki-benchmarks/std-smoke.json
uv run --directory kits/python-runner python benchmarks/performance_model.py \
  --mode smoke --output ../../target/mutsuki-benchmarks/python-runner-smoke.json
```

Local/public runs are environment-specific evidence. A release baseline requires an explicit
approval whose report hash, revision snapshot and environment fingerprint match. See
[performance/README.md](performance/README.md) and
[Epic #35 acceptance](docs/issue35-acceptance.md).

## Release and migration

- [Release train](docs/release-train.md)
- [v0.1.0 compatibility matrix](docs/compatibility/v0.1.0.md)
- [Issue #44 migration ledger](docs/migration/issue-44-ledger.md)
- [Monorepo decision](docs/decisions/0001-mutsuki-monorepo.md)

Business products such as Lilia and Nana remain in their own repositories and pin this repository
at a tag or commit. The
[independent Bot GitHub Template](https://github.com/sena-nana/MutsukiBotTemplate) is generated
from `templates/bot`; it remains active for “Use this template” but is not a second source of
framework behavior.

## Reading order

1. [AGENTS.md](AGENTS.md)
2. [plans/roadmap.md](plans/roadmap.md)
3. [plans/architecture.md](plans/architecture.md)
4. [plans/engineering.md](plans/engineering.md)
5. [plans/contracts.md](plans/contracts.md)

## License

[MIT](LICENSE)
