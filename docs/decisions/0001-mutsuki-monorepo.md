# ADR 0001: Maintain Mutsuki as a package-oriented monorepo

- Status: Accepted
- Date: 2026-07-27
- Issue: `#44`
- Qualified by: [ADR 0002](0002-first-party-bot-product.md)

## Context

Core, Link, Hosts, Kits and standard plugins share fast-moving contracts. Independent repositories
required non-atomic revision bumps, temporarily incompatible lockfiles, duplicated CI and scattered
Issues. The operational boundary was already one compatibility baseline.

## Decision

Use the renamed `Mutsuki` repository as the framework source of truth:

- retain independent Cargo packages and non-Rust packages;
- use one root Cargo Workspace and lockfile;
- import each framework repository with history under an ownership-oriented directory;
- migrate retained Issues, labels and milestones to one tracker;
- release all packages on one minor-version train while interfaces are unstable;
- keep business products external and pinned to a tag or commit;
- generate the independent Bot template from the canonical source in this repository.

## Consequences

Cross-package breaking changes can be implemented and validated atomically. CI can check dependency
direction and contract conformance in one revision. Package owners still maintain scoped rules,
skills, tests and performance evidence.

The root build is larger, so CI and local workflows should use package selection for iteration and
the full locked Workspace only as a merge/release gate. Archiving old repositories is irreversible
operationally, so it occurs only after Issue transfer, source back-links, consumer migration,
clean-clone validation and release publication.
