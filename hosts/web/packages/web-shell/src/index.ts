import {
  createRegistry,
  withExtensionBoundary,
  type Disposable,
  type ExtensionContext,
  type WebExtension,
} from "@mutsuki/web-sdk";

export interface ShellState {
  extensions: string[];
  failures: Array<{ extensionId: string; message: string }>;
  pages: ReturnType<typeof createRegistry<{ id: string; path: string; title: string; component: unknown }>>;
  navigation: ReturnType<typeof createRegistry<{ id: string; label: string; path: string }>>;
  slots: ReturnType<typeof createRegistry<{ id: string; slot: string; component: unknown }>>;
  commands: ReturnType<typeof createRegistry<{ id: string; title: string; run: () => void | Promise<void> }>>;
}

export function createShellState(): ShellState {
  return {
    extensions: [],
    failures: [],
    pages: createRegistry(),
    navigation: createRegistry(),
    slots: createRegistry(),
    commands: createRegistry(),
  };
}

/** Load precompiled ESM extensions through import maps. No runtime Vue SFC compile. */
export async function loadExtensions(
  state: ShellState,
  urls: Array<{ id: string; url: string }>,
  ctxFactory: (state: ShellState) => ExtensionContext,
): Promise<Disposable[]> {
  const disposables: Disposable[] = [];
  for (const item of urls) {
    try {
      const mod = (await import(/* @vite-ignore */ item.url)) as {
        default: WebExtension;
      };
      const extension = mod.default;
      const ctx = ctxFactory(state);
      withExtensionBoundary(
        item.id,
        () => {
          const result = extension.setup(ctx);
          if (result && typeof (result as Disposable).dispose === "function") {
            disposables.push(result as Disposable);
          }
          state.extensions.push(item.id);
        },
        (extensionId, error) => {
          state.failures.push({
            extensionId,
            message: error instanceof Error ? error.message : String(error),
          });
        },
      );
    } catch (error) {
      state.failures.push({
        extensionId: item.id,
        message: error instanceof Error ? error.message : String(error),
      });
    }
  }
  return disposables;
}

export function createExtensionContext(state: ShellState, rpc: ExtensionContext["rpc"], events: ExtensionContext["events"]): ExtensionContext {
  return {
    pages: state.pages,
    navigation: state.navigation,
    slots: state.slots,
    commands: state.commands,
    rpc,
    events,
  };
}

/** Shared runtimes provided by the shell — plugins must externalize these. */
export const SHARED_IMPORT_MAP = {
  vue: "/shared/vue.js",
  "vue-router": "/shared/vue-router.js",
  pinia: "/shared/pinia.js",
  "@mutsuki/web-sdk": "/shared/web-sdk.js",
  "@mutsuki/ui": "/shared/ui.js",
} as const;

export {
  applyTheme,
  resolveTheme,
  ConsoleShell,
  createConsoleShellElement,
  type ConsoleNavItem,
  type MutsukiTheme,
} from "@mutsuki/ui";
