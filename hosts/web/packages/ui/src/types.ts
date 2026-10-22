import type { Ref } from "vue";

export interface MkOption<V extends string | number = string> {
  value: V;
  label: string;
  disabled?: boolean;
}

export interface MkTab {
  id: string;
  label: string;
  disabled?: boolean;
}

export interface MkColumn<R> {
  key: string;
  label: string;
  align?: "start" | "end" | "center";
  sortable?: boolean;
  /** Text for a cell; defaults to the row value. A `cell-<key>` slot overrides it. */
  format?: (row: R) => string;
}

export interface MkKeyValueItem {
  label: string;
  value: string | number | null | undefined;
  mono?: boolean;
}

export type MkTone = "ok" | "warn" | "err" | "info" | "neutral";

export interface MkTimelineItem {
  id: string;
  title: string;
  description?: string;
  time?: Date | number | string;
  tone?: MkTone;
}

/** Structural `RpcError`: code, message and whether retrying can help. */
export interface MkErrorLike {
  code?: string;
  message: string;
  retryable?: boolean;
}

type MaybeRef<V> = V | Ref<V> | Readonly<Ref<V>>;

/** The shape of `useRpc`'s result, structurally, so the UI does not own query logic. */
export interface MkAsyncSource<V> {
  data: MaybeRef<V | undefined>;
  error: MaybeRef<MkErrorLike | null>;
  loading: MaybeRef<boolean>;
  refresh?: () => unknown;
}

export interface MkMenuItem {
  id: string;
  label: string;
  danger?: boolean;
  disabled?: boolean;
}
