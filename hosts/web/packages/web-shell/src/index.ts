export * from "./runtime.js";

/** Shared UI runtime provided by the shell — extensions must externalize it. */
export {
  applyTheme,
  resolveTheme,
  ConsoleShell,
  createConsoleShellElement,
  type ConsoleNavItem,
  type MutsukiTheme,
} from "@mutsuki/ui";
