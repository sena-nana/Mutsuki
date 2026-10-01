#!/usr/bin/env bash
set -euo pipefail

# Every pattern below scans `.`, so the scope is whatever directory the caller
# happens to be in. Pin it to this package the way the sibling boundary scripts
# do: from the repository root the same patterns match 28 ordinary workspace
# paths and the check fails for no reason.
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

# Each check below is an `if rg ...; then fail; fi`. A missing `rg` exits 127,
# which reads as "no matches", and `set -e` does not fire inside an `if`
# condition -- so without this guard the script reports success having scanned
# nothing. Fail loudly instead.
if ! command -v rg >/dev/null 2>&1; then
  echo "boundary check needs ripgrep (rg); install it and re-run" >&2
  exit 1
fi

if rg -n '^\[patch\.' --glob 'Cargo.toml' --glob '**/Cargo.toml' .; then
  echo 'external Cargo patches are forbidden' >&2
  exit 1
fi

if rg -n 'path\s*=\s*"\.\./' --glob 'Cargo.toml' --glob '**/Cargo.toml' .; then
  echo 'Cargo paths outside this repository are forbidden' >&2
  exit 1
fi

if rg -n 'mutsuki-service-(runtime|host)' --glob 'Cargo.toml' --glob '**/Cargo.toml' .; then
  echo 'DistributedHost may consume ServiceHost control/ipc crates, not its runtime' >&2
  exit 1
fi

echo 'distributed ownership boundaries passed'
