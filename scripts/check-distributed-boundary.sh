#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# Every check below is an `if rg ...; then fail; fi`. A missing `rg` exits 127,
# which reads as "no matches" -- and `set -e` does not fire inside an `if`
# condition -- so without this guard the script prints "passed" having scanned
# nothing at all. Fail loudly instead of reporting a vacuous green.
if ! command -v rg >/dev/null 2>&1; then
  echo "distributed boundary check needs ripgrep (rg); install it and re-run" >&2
  exit 1
fi

forbidden_dependency='(^[[:space:]]*[[:alnum:]_.-]*(cluster|distributed|consensus|quorum|remote[-_]resource|trust|attestation|openraft|raft|libp2p|quinn|tonic|transport)[[:alnum:]_.-]*[[:space:]]*=)|(package[[:space:]]*=[[:space:]]*"[[:alnum:]_.-]*(cluster|distributed|consensus|quorum|remote[-_]resource|trust|attestation|openraft|raft|libp2p|quinn|tonic|transport)[[:alnum:]_.-]*")'
if rg -n -i "$forbidden_dependency" crates/mutsuki-runtime-{contracts,core,sdk,sdk-macros}/Cargo.toml; then
  echo "distributed boundary violation: forbidden cluster/transport dependency" >&2
  exit 1
fi

source_roots=(
  crates/mutsuki-runtime-contracts
  crates/mutsuki-runtime-core
  crates/mutsuki-runtime-sdk
  crates/mutsuki-runtime-sdk-macros
  kits/agent/crates/mutsuki-agent-contracts
  kits/agent/crates/mutsuki-agent-runtime
  kits/agent/crates/mutsuki-agent-sdk
  hosts/service
  hosts/tauri
  hosts/web
)

forbidden_types='\b(NodeId|ClusterId|ClusterContext|AssignmentLease|ExecutionGrant|GlobalTaskId|TrustLevel|Leader|Follower|CoordinatorLease)\b'
if rg -n --glob '*.rs' --glob '*.h' "$forbidden_types" "${source_roots[@]}"; then
  echo "distributed boundary violation: cluster-only type leaked into Core/contracts/SDK/default Agent/ordinary Host" >&2
  exit 1
fi

distributed_feature='cfg(_attr)?\s*\([^\n]*(feature\s*=\s*"distributed"|feature\s*=\s*"cluster")'
if rg -n --glob '*.rs' --glob '*.h' "$distributed_feature" "${source_roots[@]}"; then
  echo "distributed boundary violation: plugin/runtime feature fork detected" >&2
  exit 1
fi

echo "distributed boundary checks passed"
