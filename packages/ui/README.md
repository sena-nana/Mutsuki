# `@mutsuki/ui`

LiliaUI-aligned shared styles and console chrome for Mutsuki WebHost.

- **Styles**: sourced from the pinned dependency `@lilia/theme`
  (`github:sena-nana/LiliaUI#workspace=@lilia/theme&commit=<sha>`, the single
  visual source of truth). `scripts/bundle-css.mjs` resolves `@lilia/theme`
  from `node_modules` and inlines `base.css` + layout CSS, then appends the
  product-only `console.css`, into `dist/mutsuki-ui.css`. No Lilia CSS is
  copied into this package anymore.
- **API**: `applyTheme` / `resolveTheme`, `ConsoleShell` (Vue),
  `createConsoleShellElement` (vanilla).
- **Consumers**: Web shell import map (`@mutsuki/ui` → `/shared/ui.js`);
  Bot console materializes the bundled CSS (see BotPlugins
  `scripts/sync-mutsuki-ui-css.sh`).
- **Fonts**: `base.css` declares `@font-face` with host-served `/fonts/*.woff2`
  paths; the host must serve those (falls back to system fonts otherwise).

To bump the Lilia visual base, re-pin `@lilia/theme` to a new LiliaUI commit
and rebuild — never hand-edit vendored CSS.
