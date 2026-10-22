# `@mutsuki/ui`

Lilia-aligned styles and console chrome for Mutsuki WebHost.

- **Styles**: pinned `@lilia/theme` via pnpm `path:` subdir. `scripts/bundle-css.mjs` inlines base + layout + product `console.css` into `dist/mutsuki-ui.css`.
- **API**: `applyTheme` / `resolveTheme`, `ConsoleShell` (Vue), `createConsoleShellElement` (vanilla).
- **Consumers**: `@mutsuki/web-runtime` builds `ui.js` (Vue external) and copies the CSS to
  `browser/shared/ui.css`; `mutsuki-web-shared-runtime` embeds both once and the console links
  the stylesheet under a content-addressed name. No crate vendors its own copy.
- **Fonts**: `@font-face` uses host-served `/fonts/*.woff2` (system fallback otherwise).

Bump Lilia by re-pinning `@lilia/theme` and rebuilding — do not hand-edit vendored CSS.
