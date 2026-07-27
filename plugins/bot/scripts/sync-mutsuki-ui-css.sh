#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="${MUTSUKI_UI_CSS:-$ROOT/../MutsukiWebHost/packages/ui/dist/mutsuki-ui.css}"
if [[ ! -f "$SRC" ]]; then
  echo "missing $SRC — build @mutsuki/ui first (pnpm --filter @mutsuki/ui build)" >&2
  exit 1
fi
BANNER="/* Synced from @mutsuki/ui dist/mutsuki-ui.css — run: scripts/sync-mutsuki-ui-css.sh */"
for dest in \
  "$ROOT/crates/mutsuki-bot-web-console/assets/mutsuki-ui.css" \
  "$ROOT/crates/mutsuki-plugin-bot-overview-web/assets/mutsuki-ui.css" \
  "$ROOT/crates/mutsuki-plugin-bot-config-web/assets/mutsuki-ui.css"
do
  { echo "$BANNER"; cat "$SRC"; } > "$dest"
  echo "wrote $dest"
done
