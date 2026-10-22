import { inject, reactive, type App, type InjectionKey } from "vue";
import type { MkTone } from "../types";

export interface ConfirmOptions {
  title: string;
  message?: string;
  confirmLabel?: string;
  cancelLabel?: string;
  /** Destructive action: danger styling and an alertdialog. */
  danger?: boolean;
  /** The user must type this exact text before confirming (irreversible actions). */
  requireText?: string;
}

export interface PromptOptions {
  title: string;
  label: string;
  message?: string;
  initial?: string;
  placeholder?: string;
  confirmLabel?: string;
  /** Returns an error message, or null when the value is acceptable. */
  validate?: (value: string) => string | null;
}

export interface ToastOptions {
  message: string;
  tone?: MkTone;
  /** 0 keeps it until dismissed. Errors default to 0. */
  timeoutMs?: number;
}

export type OverlayRequest =
  | { id: number; kind: "confirm"; options: ConfirmOptions; resolve: (value: boolean) => void }
  | { id: number; kind: "prompt"; options: PromptOptions; resolve: (value: string | null) => void };

export interface ToastEntry {
  id: number;
  message: string;
  tone: MkTone;
}

/** Queue behind the imperative overlay APIs, rendered by one `MkOverlayHost`. */
export interface OverlayStore {
  readonly requests: OverlayRequest[];
  readonly toasts: ToastEntry[];
  confirm(options: ConfirmOptions): Promise<boolean>;
  prompt(options: PromptOptions): Promise<string | null>;
  toast(options: ToastOptions): number;
  dismissToast(id: number): void;
  settle(id: number, value: boolean | string | null): void;
}

/** Toasts kept at once; the oldest goes first. */
export const MAX_TOASTS = 5;

export function createOverlayStore(): OverlayStore {
  let sequence = 0;
  const state = reactive({ requests: [] as OverlayRequest[], toasts: [] as ToastEntry[] });
  const timers = new Map<number, ReturnType<typeof setTimeout>>();
  const store: OverlayStore = {
    get requests() {
      return state.requests;
    },
    get toasts() {
      return state.toasts;
    },
    confirm(options) {
      return new Promise((resolve) => {
        state.requests.push({ id: ++sequence, kind: "confirm", options, resolve });
      });
    },
    prompt(options) {
      return new Promise((resolve) => {
        state.requests.push({ id: ++sequence, kind: "prompt", options, resolve });
      });
    },
    toast(options) {
      const id = ++sequence;
      const tone = options.tone ?? "info";
      state.toasts.push({ id, message: options.message, tone });
      while (state.toasts.length > MAX_TOASTS) store.dismissToast(state.toasts[0].id);
      const timeout = options.timeoutMs ?? (tone === "err" ? 0 : 5_000);
      if (timeout > 0) timers.set(id, setTimeout(() => store.dismissToast(id), timeout));
      return id;
    },
    dismissToast(id) {
      clearTimeout(timers.get(id));
      timers.delete(id);
      const index = state.toasts.findIndex((toast) => toast.id === id);
      if (index >= 0) state.toasts.splice(index, 1);
    },
    settle(id, value) {
      const index = state.requests.findIndex((request) => request.id === id);
      if (index < 0) return;
      const [request] = state.requests.splice(index, 1);
      if (request.kind === "confirm") request.resolve(value === true);
      else request.resolve(typeof value === "string" ? value : null);
    },
  };
  return store;
}

export const OVERLAY_STORE: InjectionKey<OverlayStore> = Symbol("mk.overlays");

/** Provides the overlay queue; mount `MkOverlayHost` once inside the same app. */
export function installOverlays(app: App, store: OverlayStore = createOverlayStore()): OverlayStore {
  app.provide(OVERLAY_STORE, store);
  return store;
}

function useStore(): OverlayStore {
  const store = inject(OVERLAY_STORE, null);
  if (!store) {
    throw new Error("no overlay host: call installOverlays(app) and mount <MkOverlayHost />");
  }
  return store;
}

/** Replaces `window.confirm`: resolves true only on an explicit confirmation. */
export function useConfirm(): (options: ConfirmOptions) => Promise<boolean> {
  const store = useStore();
  return (options) => store.confirm(options);
}

/** Replaces `window.prompt`: resolves the entered value, or null when cancelled. */
export function usePrompt(): (options: PromptOptions) => Promise<string | null> {
  const store = useStore();
  return (options) => store.prompt(options);
}

export interface Toaster {
  show(options: ToastOptions): number;
  success(message: string): number;
  info(message: string): number;
  warn(message: string): number;
  error(message: string): number;
  dismiss(id: number): void;
}

export function useToast(): Toaster {
  const store = useStore();
  return {
    show: (options) => store.toast(options),
    success: (message) => store.toast({ message, tone: "ok" }),
    info: (message) => store.toast({ message, tone: "info" }),
    warn: (message) => store.toast({ message, tone: "warn" }),
    error: (message) => store.toast({ message, tone: "err" }),
    dismiss: (id) => store.dismissToast(id),
  };
}
