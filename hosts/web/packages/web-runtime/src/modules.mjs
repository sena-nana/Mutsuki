/**
 * The shared runtime: one browser module per import-map specifier, plus the design-system
 * stylesheet. `mutsuki-web-shared-runtime` embeds exactly these files.
 */
export const SHARED_MODULES = [
  { specifier: "vue", file: "vue.js" },
  { specifier: "vue-router", file: "vue-router.js" },
  { specifier: "pinia", file: "pinia.js" },
  { specifier: "@mutsuki/web-sdk", file: "web-sdk.js" },
  { specifier: "@mutsuki/ui", file: "ui.js" },
  { specifier: "@mutsuki/web-shell", file: "web-shell.js" },
];

export const SHARED_STYLESHEET = "ui.css";

/** Budgets for the shared runtime itself; extensions keep the stricter web-build defaults. */
export const SHARED_BUDGETS = {
  jsBytes: 200 * 1024,
  // Lilia base + workspace/sidebar/page layouts + console chrome + node editor, unminified so
  // the font-face urls stay byte-for-byte what the host serves.
  cssBytes: 160 * 1024,
};
