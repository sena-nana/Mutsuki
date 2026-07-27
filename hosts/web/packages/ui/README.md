# `@mutsuki/ui`

Lilia-aligned styles and console chrome for Mutsuki WebHost.

- **Styles**: pinned `@lilia/theme` via pnpm `path:` subdir. `scripts/bundle-css.mjs` inlines base + layout + product `console.css` into `dist/mutsuki-ui.css`.
- **API**: `applyTheme` / `resolveTheme`, `ConsoleShell` (Vue), `createConsoleShellElement` (vanilla).
- **Consumers**: web-shell import map; BotPlugins `scripts/sync-mutsuki-ui-css.sh`.
- **Fonts**: `@font-face` uses host-served `/fonts/*.woff2` (system fallback otherwise).

Bump Lilia by re-pinning `@lilia/theme` and rebuilding — do not hand-edit vendored CSS.
