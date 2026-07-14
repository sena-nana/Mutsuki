#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

forbidden_dependency='(^[[:space:]]*[[:alnum:]_.-]*(cluster|distributed|consensus|quorum|remote[-_]resource|trust|attestation|openraft|raft|libp2p|quinn|tonic|transport)[[:alnum:]_.-]*[[:space:]]*=)|(package[[:space:]]*=[[:space:]]*"[[:alnum:]_.-]*(cluster|distributed|consensus|quorum|remote[-_]resource|trust|attestation|openraft|raft|libp2p|quinn|tonic|transport)[[:alnum:]_.-]*")'
if rg -n -i "$forbidden_dependency" Cargo.toml crates/*/Cargo.toml; then
  echo "distributed boundary violation: forbidden cluster/transport dependency" >&2
  exit 1
fi

source_roots=(
  crates/mutsuki-runtime-contracts
  crates/mutsuki-runtime-core
  crates/mutsuki-runtime-sdk
  crates/mutsuki-runtime-sdk-macros
)

forbidden_types='\b(NodeId|ClusterId|ClusterContext|AssignmentLease|ExecutionGrant|GlobalTaskId|TrustLevel|Leader|Follower)\b'
if rg -n --glob '*.rs' --glob '*.h' "$forbidden_types" "${source_roots[@]}"; then
  echo "distributed boundary violation: cluster-only type leaked into Core/contracts/SDK" >&2
  exit 1
fi

distributed_feature='cfg(_attr)?\s*\([^\n]*(feature\s*=\s*"distributed"|feature\s*=\s*"cluster")'
if rg -n --glob '*.rs' --glob '*.h' "$distributed_feature" "${source_roots[@]}"; then
  echo "distributed boundary violation: plugin/runtime feature fork detected" >&2
  exit 1
fi

echo "distributed boundary checks passed"
