# Std Plugin Migration Notes

This document tracks runtime capabilities that must live in standard plugins
instead of becoming permanent core behavior.

## Migrated This Pass

- The former `resource.local` provider surface is no longer used by core tests
  or load-plan fixtures.
- Core resource tests register explicit provider-backed descriptors instead of
  deriving a default provider id from `ResourceManager`.
- `MutsukiStdPlugins` now has a concrete `mutsuki-plugin-resource-memory`
  provider for the standard put/get/stat/clone/drop and lease/release baseline,
  so the next core removal has an actual provider destination.
- `mutsuki-plugin-resource-memory` also implements the SDK `ResourcePlanGateway`
  baseline for collect/export/snapshot/write commit/command/batch/saga, so host
  resource plan execution is owned by the std provider boundary.
- `LocalResourceClient` and the `HostRuntimeCommand` resource plan control path
  now execute through the host-side `ResourceProviderGateway` boundary, which
  covers both resource creation and resource plan execution.
- `RuntimeBootstrapper` now collects `ResourceProviderGateway` instances from
  `LoadedPlugin` and injects the active provider selected by the load plan into
  `HostRuntimeConfig`. A manifest-only active provider fails at host boot
  instead of silently falling back to the core compatibility bridge.
- `HostRuntimeCommand` resource creation and plan execution now require an
  injected `ResourceProviderGateway`; without one they fail loudly with a
  structured provider-missing error instead of using core-managed storage.
- `LocalResourceClient` no longer exposes a default core-backed constructor or
  core compatibility provider. Callers must inject a `ResourceProviderGateway`.
- Host resource-client and plugin-backend grouping tests now inject test
  providers directly, so they no longer exercise core resource storage as the
  default host behavior.
- `CoreRuntime` no longer exposes resource plan data-plane facade methods for
  collect, snapshot, export, command, batch, saga, or write commit. Host/plugin
  paths must use `ResourceProviderGateway`; `ResourceManager` only keeps
  descriptor, generation, lease, occupancy, and plan-construction facts.
- `ResourceManager` no longer owns inline UTF-8 export or deterministic query
  command execution. Those plan behaviors are covered by
  `mutsuki-plugin-resource-memory`.
- `CoreResourcePlanProvider` and `LocalResourceClient::with_core_compat_provider`
  have been removed; host resource clients cannot silently execute resource
  plan data-plane work against core storage.
- `CoreRuntime` no longer exposes blob/mmap/COW resource creation, raw resource
  read, or bytes write facades. Runtime-level tests that only need live
  resource facts now register external resource descriptors, keeping core
  responsible for descriptor, lease and occupancy checks rather than provider
  bytes storage.
- `ResourceManager` no longer owns `LocalResourceStore` or any local
  bytes/blob/mmap creation, read, copy-on-write, snapshot, fact encoding, or
  bytes write primitive. Core resource tests now register provider-backed
  descriptors and validate descriptor routing, lazy plan construction, lease
  fencing and occupancy facts.

## Remaining Rust Core Boundary Work

No Rust core bytes/blob/mmap provider implementation remains. The remaining
core-adjacent resource work is to define how provider commit receipts update or
replace registered descriptors after host-side `ResourceProviderGateway`
execution, without reintroducing core-managed bytes storage.

## Must Stay In Core

- `ResourceManager` descriptor registry, generation checks, and occupancy facts
- `ResourceCellRef` and `ResourceLease` lifecycle/fencing
- `ReadPlan`, `WritePlan`, `CommandPlan`, and related pure wire contracts
- hot-reload occupancy checks for resource providers, resource types, streams,
  subscriptions, and timers

## Must Stay Out Of Core

- resource provider bytes/blob/mmap implementation
- workflow linear/broadcast orchestration
- filesystem, HTTP, SQLite, permission, config, log, trace, and dev-mock
  behavior
- any business protocol or product-specific resource kind
