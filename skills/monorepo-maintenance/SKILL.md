---
name: monorepo-maintenance
description: Maintain the Mutsuki monorepo package map, internal Cargo dependencies, repository migrations, first-party products, release train, compatibility matrix, and archive ledger. Use for directory moves, package additions, dependency-boundary changes, migrated Issue work, unified releases, or changes that previously crossed Mutsuki repositories.
---

# Monorepo Maintenance

Keep one repository and one Rust compatibility baseline without collapsing package ownership.

## Workflow

1. Read the root `AGENTS.md`, `plans/{roadmap,architecture,engineering,contracts}.md`, and the
   closest scoped `AGENTS.md`.
2. Read [package-map.md](references/package-map.md) and the scoped skill for every affected package.
3. Use local workspace dependencies for packages in this repository. Never add a Git dependency
   from one Mutsuki package to another.
4. Preserve package names, public boundaries, tests, docs, and history during moves. A directory
   group is not a feature-combined facade crate.
5. Update the migration ledger, architecture decision, compatibility matrix, release notes, and
   first-party Bot product whenever their facts change.
6. Run `python3 skills/monorepo-maintenance/scripts/check_workspace.py` before Cargo validation.
7. Validate the smallest affected package set, then the locked full workspace, Python kit,
   JavaScript packages, integration tests, first-party product acceptance, and owner performance suites.
8. Tag and publish one release-train revision only after all package and consumer gates pass.

## Git And Issue Migration

- Import source history with a merge/subtree strategy; do not copy only the latest snapshot.
- Transfer every open Issue and every retained closed Issue. Preserve source links, authorship,
  comments, labels, milestones, and package ownership.
- Add a migration notice to each source repository before archiving it.
- Do not archive a repository while executable work exists only in that repository.
- Archive retired generated distribution repositories after their notice points to the canonical
  in-tree product and no executable work remains there.
- Keep `products/bot` as the first-party product exception. Keep other business products outside
  this repository and pin them to a released tag or commit.

## Validation

Treat a successful root build as necessary but insufficient. Report exact commands and results for
Rust, Python, frontend, integration, performance, first-party products, clean clone, and external
consumer checks. Fail when any selected capability, artifact, package, or revision is unavailable.
