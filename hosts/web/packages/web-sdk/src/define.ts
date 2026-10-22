import type { Component } from "vue";

import type { Disposable, ExtensionContext, Registry, SlotRegistration, WebExtension } from "./extension.js";
import { RpcError } from "./errors.js";
import type { RpcClient } from "./bridge.js";

/** A vanilla mount point, the contract every slot accepts until Vue slots land. */
export interface MountableComponent {
  mount(element: HTMLElement, options?: Record<string, unknown>): void | Disposable | Promise<void | Disposable>;
}

/**
 * Slot payload types, keyed by slot name. Owners add their slots by declaration merging:
 * `declare module "@mutsuki/web-sdk" { interface ExtensionSlots { "flow.node": … } }`.
 */
export interface ExtensionSlots {
  "overview.cards": MountableComponent | Component;
  "plugin.home": MountableComponent;
  /**
   * Extra panel in the platforms account detail, named by the adapter descriptor's
   * `detail_panels` (e.g. `platform.detail:bilibili`). Rendered with `{ adapter, account }` props.
   */
  [panel: `platform.detail:${string}`]: Component;
}

export type SlotName = keyof ExtensionSlots & string;

export type TypedSlotRegistration<K extends SlotName = SlotName> = {
  [S in K]: Omit<SlotRegistration, "slot" | "component"> & {
    slot: S;
    component: ExtensionSlots[S];
  };
}[K];

export interface DefinedExtensionContext extends Omit<ExtensionContext, "slots"> {
  readonly extensionId: string;
  slots: Registry<SlotRegistration> & {
    register<K extends SlotName>(item: TypedSlotRegistration<K>): Disposable;
  };
}

/** Per-extension limits. Exceeding one fails the extension, never the shell. */
export interface ExtensionBudgets {
  setupTimeoutMs: number;
  /** Pages, navigation, slots, commands and activities together. */
  maxRegistrations: number;
  maxConcurrentRpc: number;
}

export const DEFAULT_EXTENSION_BUDGETS: ExtensionBudgets = {
  setupTimeoutMs: 5_000,
  maxRegistrations: 128,
  maxConcurrentRpc: 16,
};

/** Shell-wide cap on loaded extensions. */
export const MAX_EXTENSIONS = 64;

export interface ExtensionDefinition {
  /** Stable owner id, e.g. `mutsuki.web.overview`. */
  id: string;
  budgets?: Partial<ExtensionBudgets>;
  setup(ctx: DefinedExtensionContext): void | Disposable | Promise<void | Disposable>;
}

const EXTENSION_ID = /^[a-z][a-z0-9-]*(\.[a-z][a-z0-9-]*)*$/;

function countedRegistry<T>(registry: Registry<T>, budget: { left: number }, extensionId: string): Registry<T> {
  return {
    register(item) {
      if (budget.left <= 0) {
        throw new RpcError(
          "budget_exceeded",
          `extension ${extensionId} exceeded its registration budget`,
          { retryable: false },
        );
      }
      budget.left -= 1;
      return registry.register(item);
    },
    list: () => registry.list(),
  };
}

function limitedRpc(rpc: RpcClient, limit: number, extensionId: string): RpcClient {
  let active = 0;
  const call: RpcClient["call"] = async (namespace, method, params, options) => {
    if (active >= limit) {
      throw new RpcError(
        "pending_limit",
        `extension ${extensionId} has ${limit} RPCs in flight`,
      );
    }
    active += 1;
    try {
      return await rpc.call(namespace, method, params, options);
    } finally {
      active -= 1;
    }
  };
  return {
    call,
    read: (namespace, method, params) => call(namespace, method, params, { operation: "read" }),
    write: (namespace, method, params) => call(namespace, method, params, { operation: "write" }),
  };
}

/**
 * Declares an extension with typed slots and enforced budgets: a bounded registration count,
 * a bounded number of concurrent RPCs and a setup deadline.
 */
export function defineExtension(definition: ExtensionDefinition): WebExtension {
  if (!EXTENSION_ID.test(definition.id)) {
    throw new RpcError("invalid_extension", `invalid extension id: ${definition.id}`, {
      retryable: false,
    });
  }
  const budgets = { ...DEFAULT_EXTENSION_BUDGETS, ...definition.budgets };
  return {
    id: definition.id,
    async setup(ctx) {
      const registrations = { left: budgets.maxRegistrations };
      const bounded = <T>(registry: Registry<T>) =>
        countedRegistry(registry, registrations, definition.id);
      const context: DefinedExtensionContext = {
        ...ctx,
        extensionId: definition.id,
        activities: bounded(ctx.activities),
        pages: bounded(ctx.pages),
        navigation: bounded(ctx.navigation),
        slots: bounded(ctx.slots) as DefinedExtensionContext["slots"],
        commands: bounded(ctx.commands),
        rpc: limitedRpc(ctx.rpc, budgets.maxConcurrentRpc, definition.id),
      };
      let timer: ReturnType<typeof setTimeout> | undefined;
      const deadline = new Promise<never>((_, reject) => {
        timer = setTimeout(
          () =>
            reject(
              new RpcError(
                "extension_setup_timeout",
                `extension ${definition.id} setup exceeded ${budgets.setupTimeoutMs}ms`,
                { retryable: false },
              ),
            ),
          budgets.setupTimeoutMs,
        );
      });
      try {
        return await Promise.race([Promise.resolve(definition.setup(context)), deadline]);
      } finally {
        clearTimeout(timer);
      }
    },
  };
}
