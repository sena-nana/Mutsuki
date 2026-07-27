# Issue #44 migration ledger

## Source history

| Source repository | Imported source revision | Monorepo path | Issues total | Open | Closed | Issue transfer | Source archive |
| --- | --- | --- | ---: | ---: | ---: | --- | --- |
| MutsukiLink | `75f5f3820d29b2c58bbba6408d68bc77c74a8ec0` | `crates/link` | 14 | 2 | 12 | Pending | Pending |
| MutsukiCliHost | `681e4fd61172dcb0d7f33cf9f4e39d05f161085d` | `hosts/cli` | 0 | 0 | 0 | N/A | Pending |
| MutsukiServiceHost | `9c08382ffe00a181633549ba7b9d81222a669bd6` | `hosts/service` | 14 | 1 | 13 | Pending | Pending |
| MutsukiTauriHost | `c2920e2c9dd6416e59b6224151187c001915e83c` | `hosts/tauri` | 6 | 0 | 6 | Pending | Pending |
| MutsukiWebHost | `f24a1325684d55aacdf409cd668b0b68f3cf787d` | `hosts/web` | 1 | 0 | 1 | Pending | Pending |
| MutsukiDistributedHost | `ba8c7150195312377ca3d6a30d22f3468c9ced8c` | `hosts/distributed` | 17 | 0 | 17 | Pending | Pending |
| MutsukiAgentKit | `6a8b6d90f02fa06c68067dd29d795a382ec4b7a0` | `kits/agent` | 33 | 29 | 4 | Pending | Pending |
| MutsukiBotPlugins | `c9152c8e8e05f313343cc836985d118732785374` | `plugins/bot` | 20 | 10 | 10 | Pending | Pending |
| MutsukiStdPlugins | `382b6984129fc09b10cc8e5f3af3649fd353f9ea` | `plugins/std` | 4 | 0 | 4 | Pending | Pending |
| MutsukiPythonRunnerKit | `c48f4c4a55aacbf76066baf3a9dee945d959de9c` | `kits/python-runner` | 3 | 0 | 3 | Pending | Pending |
| MutsukiBotTemplate | `50f40fd5c357e50263b3696f2971fb0721998aa8` | `templates/bot` | 5 | 0 | 5 | Pending | Pending |

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
- a clean clone and generated template pass validation.
