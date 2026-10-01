#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORKSPACE_ROOT="$(git -C "$ROOT" rev-parse --show-toplevel)"
SRC="${MUTSUKI_UI_CSS:-$WORKSPACE_ROOT/hosts/web/packages/ui/dist/mutsuki-ui.css}"
if [[ ! -f "$SRC" ]]; then
  echo "missing $SRC — build @mutsuki/ui first (pnpm --filter @mutsuki/ui build)" >&2
  exit 1
fi
BANNER="/* Synced from @mutsuki/ui dist/mutsuki-ui.css — run: scripts/sync-mutsuki-ui-css.sh */"
for dest in \
  "$ROOT/crates/mutsuki-bot-web-host-integration/assets/mutsuki-ui.css" \
  "$WORKSPACE_ROOT/hosts/web/crates/mutsuki-web-extension-overview/assets/mutsuki-ui.css" \
  "$WORKSPACE_ROOT/plugins/std/crates/mutsuki-std-web-extension-config/assets/mutsuki-ui.css"
do
  { echo "$BANNER"; cat "$SRC"; } > "$dest"
  echo "wrote $dest"
done
