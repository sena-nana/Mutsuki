import type { Component } from "vue";
import type { EventClient, RpcClient } from "./bridge.js";

export interface Disposable {
  dispose(): void;
}


/** Owns reversible extension registrations with Scope-compatible lifecycle semantics. */
export class DisposableScope implements Disposable {
  private effects: Disposable[] = [];
  private closed = false;
  private disposed = false;

  own(effect: Disposable): Disposable {
    if (this.closed) {
      effect.dispose();
      throw new Error("disposable scope is already disposed");
    }
    let active = true;
    const owned = {
      dispose() {
        if (!active) return;
        effect.dispose();
        active = false;
      },
    };
    this.effects.push(owned);
    return owned;
  }

  dispose(): void {
    if (this.disposed) return;
    this.closed = true;
    const failures: unknown[] = [];
    const failedEffects: Disposable[] = [];
    for (const effect of this.effects.splice(0).reverse()) {
      try {
        effect.dispose();
      } catch (error) {
        failures.push(error);
        failedEffects.push(effect);
      }
    }
    this.effects = failedEffects.reverse();
    this.disposed = failures.length === 0;
    if (failures.length > 0) {
      throw new AggregateError(failures, "extension scope disposal failed");
    }
  }
}

export interface PageRegistration {
  id: string;
  path: string;
  title: string;
  /** A Vue component, or a legacy `{ mount }` object adapted by the shell. */
  component: PageComponent | Component;
  requiredCapability?: string;
  /** Runtime plugin / config provider that owns this extra page. */
  pluginId?: string;
  /** Additional owners when one page belongs to multiple plugins. */
  pluginIds?: string[];
}

export interface PageComponent {
  mount(element: HTMLElement): void | Disposable | Promise<void | Disposable>;
}

export interface ActivityRegistration {
  id: string;
  label: string;
  icon: string;
  order?: number;
  position?: "top" | "bottom";
}

export interface NavigationRegistration {
  id: string;
  activityId: string;
  pageId: string;
  label: string;
  group?: string;
  order?: number;
  requiredCapability?: string;
  /** Target page provides no content; the route renders disabled. */
  disabled?: boolean;
}

export interface SlotRegistration {
  id: string;
  slot: string;
  component: unknown;
  requiredCapability?: string;
  /** Runtime plugin / config provider that owns this contribution. */
  pluginId?: string;
  pluginIds?: string[];
  /** Homepage `overview.cards` occupancy. The host owns cell size. */
  colSpan?: number;
  rowSpan?: number;
}

export interface CommandRegistration {
  id: string;
  title: string;
  run: () => void | Promise<void>;
  requiredCapability?: string;
}

export interface ExtensionContext {
  activities: Registry<ActivityRegistration>;
  pages: Registry<PageRegistration>;
  navigation: Registry<NavigationRegistration>;
  slots: Registry<SlotRegistration>;
  commands: Registry<CommandRegistration>;
  rpc: RpcClient;
  events: EventClient;
}

export interface Registry<T> {
  register(item: T): Disposable;
  list(): T[];
}

export interface WebExtension {
  id: string;
  setup(ctx: ExtensionContext): void | Disposable | Promise<void | Disposable>;
}


export function pluginIdsOf(item: {
  pluginId?: string;
  pluginIds?: string[];
} | null | undefined): string[] {
  return [...new Set([item?.pluginId, ...(item?.pluginIds || [])].filter(Boolean) as string[])];
}

export function pluginDisplayName(
  state: Pick<ExtensionContext, "pages" | "navigation"> | null | undefined,
  pluginId?: string | null,
): string {
  const id = String(pluginId || "");
  if (!id) return id;
  const pages = state?.pages?.list() ?? [];
  const navByPage = new Map((state?.navigation?.list() ?? []).map((item) => [item.pageId, item]));
  const named = (value?: string) => (value && value !== id ? value : undefined);

  const hubNamed = named(navByPage.get(id)?.label) || named(pages.find((page) => page.id === id)?.title);
  if (hubNamed) return hubNamed;

  for (const page of pages) {
    if (page.id === id || !pluginIdsOf(page).includes(id)) continue;
    const extra = named(navByPage.get(page.id)?.label) || named(page.title);
    if (extra) return extra;
  }
  return id;
}

export function createRegistry<T extends { id: string }>(
  options: { onDuplicate?: "throw" | "retain" } = {},
): Registry<T> & {
  list(): T[];
  clear(): void;
} {
  const onDuplicate = options.onDuplicate ?? "throw";
  const items = new Map<string, { value: T; refs: number }>();
  return {
    register(item) {
      const existing = items.get(item.id);
      if (existing) {
        if (onDuplicate === "throw") {
          throw new Error(`duplicate registration id: ${item.id}`);
        }
        existing.refs += 1;
        return {
          dispose() {
            release(item.id);
          },
        };
      }
      items.set(item.id, { value: item, refs: 1 });
      return {
        dispose() {
          release(item.id);
        },
      };
    },
    list() {
      return [...items.values()].map((entry) => entry.value);
    },
    clear() {
      items.clear();
    },
  };

  function release(id: string) {
    const current = items.get(id);
    if (!current) return;
    current.refs -= 1;
    if (current.refs <= 0) items.delete(id);
  }
}

/** Error boundary helper — one extension must not crash the shell. */
export function withExtensionBoundary(
  extensionId: string,
  run: () => void,
  onError: (extensionId: string, error: unknown) => void,
): void {
  try {
    run();
  } catch (error) {
    onError(extensionId, error);
  }
}

