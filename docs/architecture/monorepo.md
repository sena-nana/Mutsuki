# Mutsuki monorepo architecture

## Purpose

Mutsuki packages share contracts and compatibility baselines closely enough that cross-package
changes must be atomic. One repository provides that atomic boundary while package names and
runtime ownership remain separate.

## Structure

```text
Mutsuki/
├── crates/
│   ├── mutsuki-runtime-*/
│   └── link/
├── hosts/{cli,service,tauri,web,distributed}/
├── kits/{agent,python-runner}/
├── plugins/{bot,std}/
├── products/bot/
├── integration-tests/
├── performance/
└── docs/{architecture,contracts,decisions,compatibility,migration}/
```

Each imported repository remains visible as an architecture group so its history, scoped
`AGENTS.md`, skills, tests, fixtures, performance reports and package docs remain attributable.
Rust package manifests inherit one root Workspace. Python and frontend packages retain their own
package formats but release against the same repository revision.

## Dependency direction

```text
runtime contracts/wire
        ↓
runtime core
        ↓
runtime host helpers and SDK
        ↓
link / standard plugins / AgentKit / Bot packages
        ↓
service, Tauri, Web and distributed Hosts
        ↓
first-party and external products
```

The diagram is ownership-oriented, not permission to add every downward dependency:

- Core never depends on Link, a concrete Host, AgentKit, Bot packages or standard plugins.
- Link never depends on a concrete Host or business package.
- AgentKit, Bot packages and standard plugins remain Host-neutral.
- Host-specific integration lives in an explicitly named integration package.
- Products select packages but do not absorb implementation owned by Core, Hosts, Kits or plugins.
- External products choose only required packages from one release revision.

## Workspace invariants

- The root `Cargo.toml` and `Cargo.lock` are the only Rust Workspace and compatibility lock.
- Internal Mutsuki dependencies are local path dependencies inherited from the root.
- No nested Workspace, internal Git pin, repository-external path or local patch is allowed.
- Package names stay stable; no root facade crate or global feature matrix is introduced.
- A clean clone can build without former sibling repositories.

`skills/monorepo-maintenance/scripts/check_workspace.py` enforces the mechanical subset of these
rules. Cargo tests, contract conformance and scoped integration tests enforce behavior.

## Non-Rust packages

`kits/python-runner` mirrors the checked-in runtime wire artifacts from the same revision.
`hosts/tauri` and `hosts/web` own their frontend SDKs and shells. Their package locks are not Rust
Workspace locks and remain scoped to their JavaScript build boundaries.

## First-party Bot product

`products/bot` is the repository's runnable Bot product. It owns external bootstrap configuration,
owner catalog aggregation, ServiceRuntime startup, the thin external-product `create-bot` scaffold
and cross-package product acceptance. Runtime Core remains domain-neutral, ServiceHost keeps process
lifecycle ownership, and Bot, Agent and Std packages keep their protocol and implementation
ownership. The product does not introduce a `BotHost`, duplicate owner implementations or an
all-capability facade crate. Generated projects pin one immutable Mutsuki revision and call the
public product API; they are not an exported copy of the first-party product.

## History and retired repositories

Every former framework repository was merged with its Git history as a parent of the monorepo.
Issues are transferred to the unified tracker and source repositories receive a migration notice
before archival. `MutsukiBotTemplate` is being retired because its former source and generated
distribution role moved to `products/bot`; Bot source, runtime changes and Issues belong only in
this monorepo.
Exact revisions and Issue counts live in
[`docs/migration/issue-44-ledger.md`](../migration/issue-44-ledger.md).
