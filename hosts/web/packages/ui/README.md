# `@mutsuki/ui`

Mutsuki design system on top of the pinned Lilia theme.

- **Tokens**: `src/styles/tokens.css` maps Lilia variables to `--mk-*`. Components read only
  `--mk-*`; container breakpoints are compact ≤ 640px and medium ≤ 1024px (`.mk-container`).
- **Components** (Vue 3 SFC, global `mk-` classes, CSS loaded once by the shell):
  - base: `MkButton`, `MkIconButton`, `MkIcon`, `MkCard`, `MkPageHeader`, `MkSection`, `MkTabs`,
    `MkSplitPane`, `MkDrawer`
  - data: `MkDataTable`, `MkKeyValue`, `MkStatusBadge`, `MkRelativeTime`, `MkJsonView`,
    `MkCodeBlock`, `MkCopyButton`, `MkTimeline`
  - state: `MkEmptyState`, `MkErrorState`, `MkSkeleton`, `MkAsync` (takes a `useRpc` result)
  - forms: `MkField` (wires id/description/invalid into its control), `MkTextInput`,
    `MkTextarea`, `MkNumberInput`, `MkSelect`, `MkSwitch`, `MkCheckbox`, `MkRadioGroup`,
    `MkSegmented`, `MkSecretInput` (write-only), `MkSearchInput`
  - overlays: `MkDialog`, `MkMenu`, `MkTooltip`; shell a11y: `MkSkipLink`, `MkLiveRegion`
- **Interaction**: `installMutsukiUi(app)` + one `<MkOverlayHost />`, then `useConfirm()`
  (`requireText` for irreversible actions), `usePrompt()`, `useToast()`. Never `window.confirm`.
- **Copy**: built-in zh-CN in `uiMessages`; the shell may register it as the `ui` i18n namespace.
- **Build**: `vite build` (Vue and `@mutsuki/web-sdk` external) → `dist/index.js` +
  `dist/components.css`; `bundle-css.mjs` inlines Lilia base/layout + tokens + console chrome +
  node editor + components into `dist/mutsuki-ui.css`. `@mutsuki/web-runtime` serves both as
  `shared/ui.js` / `shared/ui.css`.
- **Tests**: vitest + `@vue/test-utils` + `vitest-axe` (contrast is not computed under happy-dom).
- **Fonts**: `@font-face` uses host-served `/fonts/*.woff2` (system fallback otherwise).

Bump Lilia by re-pinning `@lilia/theme` and rebuilding — never hand-edit generated CSS.
