# Issue #44 migration ledger

## Source history

| Source repository | Imported source revision | Monorepo path | Issues total | Open | Closed | Issue transfer | Source disposition |
| --- | --- | --- | ---: | ---: | ---: | --- | --- |
| MutsukiLink | `75f5f3820d29b2c58bbba6408d68bc77c74a8ec0` | `crates/link` | 14 | 2 | 12 | Completed | Archived |
| MutsukiCliHost | `681e4fd61172dcb0d7f33cf9f4e39d05f161085d` | `hosts/cli` | 0 | 0 | 0 | N/A | Archived |
| MutsukiServiceHost | `9c08382ffe00a181633549ba7b9d81222a669bd6` | `hosts/service` | 14 | 1 | 13 | Completed | Archived |
| MutsukiTauriHost | `c2920e2c9dd6416e59b6224151187c001915e83c` | `hosts/tauri` | 6 | 0 | 6 | Completed | Archived |
| MutsukiWebHost | `f24a1325684d55aacdf409cd668b0b68f3cf787d` | `hosts/web` | 1 | 0 | 1 | Completed | Archived |
| MutsukiDistributedHost | `ba8c7150195312377ca3d6a30d22f3468c9ced8c` | `hosts/distributed` | 17 | 0 | 17 | Completed | Archived |
| MutsukiAgentKit | `6a8b6d90f02fa06c68067dd29d795a382ec4b7a0` | `kits/agent` | 33 | 29 | 4 | Completed | Archived |
| MutsukiBotPlugins | `c9152c8e8e05f313343cc836985d118732785374` | `plugins/bot` | 20 | 10 | 10 | Completed | Archived |
| MutsukiStdPlugins | `382b6984129fc09b10cc8e5f3af3649fd353f9ea` | `plugins/std` | 4 | 0 | 4 | Completed | Archived |
| MutsukiPythonRunnerKit | `c48f4c4a55aacbf76066baf3a9dee945d959de9c` | `kits/python-runner` | 3 | 0 | 3 | Completed | Archived |
| MutsukiBotTemplate | `50f40fd5c357e50263b3696f2971fb0721998aa8` | `products/bot` | 5 | 0 | 5 | Completed | Retirement pending delivery |

The target Core history was synchronized through
`608e565f4af93e8226e01d5af3003071307a1654` before the unified Workspace gate. Imports use
history-preserving subtree merge commits; no repository was reduced to a file snapshot.

## Issue policy

All 117 source Issues are retained and transferred. This intentionally includes closed performance,
architecture, compatibility and migration Issues so active work, documentation and decisions never
point only to an archived tracker. GitHub transfer preserves comments and source redirects. Package
area and task-type labels are normalized in the target repository.

## Archive blockers

A source repository is not archivable until all of the following are true:

- history is reachable from the monorepo;
- retained Issues are transferred and source links resolve to the target;
- the source README points to the new path and release;
- no active PR or unique executable task remains;
- external consumers no longer pin the source repository;
- the unified release and compatibility matrix are published;
- a clean clone and the migrated product pass validation.

All blockers above were cleared before ten retired source repositories were archived.
`MutsukiBotTemplate` was temporarily retained as a generated distribution through v0.2.2. Its
remaining product and publication role is now consolidated into `products/bot`; after the main
repository migration is published, its README becomes a redirect and the repository is archived.

## Completed remote migration

- Target repository: [`sena-nana/Mutsuki`](https://github.com/sena-nana/Mutsuki).
- Validated unified revision: `387a42735ce13f105292ddc91e5f209979c529d9`.
- Release: [`v0.1.0`](https://github.com/sena-nana/Mutsuki/releases/tag/v0.1.0).
- The historical generated Bot template was published to
  [`sena-nana/MutsukiBotTemplate`](https://github.com/sena-nana/MutsukiBotTemplate) through v0.2.2;
  current Bot product source and execution live in `products/bot`.
- Issue map: [`issue-44-issue-map.json`](issue-44-issue-map.json), containing 117 unique
  source-to-target mappings. Every source Issue list is empty after transfer.
- The only open source PR, MutsukiLink Dependabot #20, was closed as superseded by the root
  `Cargo.lock` before archival.

## External consumer pins

| Consumer | Validated and pushed revision |
| --- | --- |
| LiliaGithub | `62bccd9` |
| LiliaCode | `26ec43b` |
| MomoBako | `d574fee` |
| MomoFlow | `5e5a65f` |
| NanaLive | `4d3f595` |
| NanaTracking | `164274a` |

All six consumers resolve Mutsuki packages from
`https://github.com/sena-nana/Mutsuki.git` at the unified revision. Their active manifests,
lockfiles and migration-facing documentation contain no former framework repository URL.

## Release verification

- Root workspace boundary: 113 Rust packages, one root workspace, no internal Git pins.
- Root Rust validation: formatting, locked check, strict Clippy, and 887 passing tests with 3
  ignored platform/fixture cases.
- Python Runner Kit: Ruff, Pyright and 82 tests passed.
- Web and Tauri frontend TypeScript builds passed.
- Performance smoke: nine owner reports, 390 cases, and all correctness gates passed; Core retained
  59 passing smoke gates.
- An isolated template export passed locked metadata, formatting, check, and 40 tests with 1
  ignored platform case.
- The generated template repository is byte-for-byte synchronized from that validated export;
  repository-local Git history is the only path excluded from comparison.
- A fresh clone of remote tag `v0.1.0` resolved the exact unified revision, passed locked workspace
  check, 887 tests with 3 ignored cases, and all 59 Core smoke performance gates.
