export * from "./runtime.js";
export * from "./layout.js";
export { buildShellModel, type ShellModel } from "./navigation.js";
export { isLegacyPage, legacyMount } from "./legacy.js";
export { shellMessages } from "./messages.js";
export { SETTINGS_PAGE_ID, startConsole, type ConsoleOptions } from "./console.js";

/** Shared UI runtime provided by the shell — extensions must externalize it. */
export { applyTheme, resolveTheme, type MutsukiTheme } from "@mutsuki/ui";
