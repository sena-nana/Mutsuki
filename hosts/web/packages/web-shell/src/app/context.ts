import { inject, type InjectionKey, type Ref } from "vue";

import type { ConsoleLayout, ShellFailure } from "../layout.js";
import type { ShellModel } from "../navigation.js";
import type { WebShellRuntime, WebUiThemeController } from "../runtime.js";

/** Everything the shell components share; provided once by `ConsoleRoot`. */
export interface ShellContext {
  runtime: WebShellRuntime;
  layout: ConsoleLayout;
  model: ShellModel;
  brand: string;
  theme: WebUiThemeController;
  /** Load, layout and render failures, newest last. */
  failures: Ref<ShellFailure[]>;
  reportFailure(failure: ShellFailure): void;
  logout(): Promise<void>;
}

export const SHELL_CONTEXT: InjectionKey<ShellContext> = Symbol("mutsuki.shell");

export function useShell(): ShellContext {
  const context = inject(SHELL_CONTEXT, null);
  if (!context) throw new Error("shell components must render inside ConsoleRoot");
  return context;
}
