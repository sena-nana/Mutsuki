import { useConfirm, useToast, type ConfirmOptions } from "@mutsuki/ui";
import {
  callRpc,
  toRpcError,
  useMessages,
  useMutsukiRuntime,
  usePolling,
  useRpc,
  useSubscription,
  type RpcMethodRef,
  type RpcQuery,
} from "@mutsuki/web-sdk";
import { onBeforeUnmount } from "vue";

import { controlChanged, type ControlDomain } from "./contract";
import { messages } from "./messages";

/** Domain changes arrive in bursts; one refresh per burst is enough. */
const EVENT_DEBOUNCE_MS = 50;
/** Events carry every change; this only reconciles anything a dropped event missed. */
export const RECONCILE_MS = 60_000;

/**
 * A control snapshot that refreshes when its domains change (not on every control event),
 * on reconnect and every minute.
 */
export function useControlQuery<P, R>(
  method: RpcMethodRef<P, R>,
  domains: ControlDomain[],
  params?: () => P,
): RpcQuery<R> {
  const query = useRpc(method, params);
  useDomainChanges(domains, () => void query.refresh());
  usePolling(() => query.refresh(), RECONCILE_MS, { immediate: false });
  return query;
}

/** Calls `onChange` (debounced) for control events touching one of `domains`. */
export function useDomainChanges(domains: ControlDomain[], onChange: () => void): void {
  let lastRevision = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  useSubscription(controlChanged, (payload) => {
    const revision = Number(payload?.revision ?? 0);
    const changed = Array.isArray(payload?.domains) ? payload.domains : [];
    if (revision <= lastRevision || !changed.some((domain) => domains.includes(domain as ControlDomain))) {
      return;
    }
    lastRevision = revision;
    clearTimeout(timer);
    timer = setTimeout(onChange, EVENT_DEBOUNCE_MS);
  });
  onBeforeUnmount(() => clearTimeout(timer));
}

/**
 * A confirmed control write: asks first, reports failure as a toast, and refreshes the
 * affected snapshot afterwards — also when delivery is unknown, since it may have landed.
 */
export function useControlAction() {
  const { client } = useMutsukiRuntime();
  const confirm = useConfirm();
  const toast = useToast();
  const { t } = useMessages(messages);
  return async function run<P>(
    method: RpcMethodRef<P, unknown>,
    params: P,
    options: { confirm?: ConfirmOptions; refresh?: () => unknown; success?: string } = {},
  ): Promise<boolean> {
    if (options.confirm && !(await confirm(options.confirm))) return false;
    try {
      await (callRpc as (c: typeof client, r: RpcMethodRef<P, unknown>, p: P) => Promise<unknown>)(
        client,
        method,
        params,
      );
      if (options.success) toast.success(options.success);
      return true;
    } catch (error) {
      toast.error(t("actionFailed", { message: toRpcError(error).message }));
      return false;
    } finally {
      await options.refresh?.();
    }
  };
}

/** Human label for runner / event-source / service states. */
export function useStateLabel(): (value: unknown) => string {
  const { t } = useMessages(messages);
  return (value) => {
    switch (String(value ?? "").toLowerCase()) {
      case "running":
      case "healthy":
      case "ok":
        return t("state.running");
      case "starting":
        return t("state.starting");
      case "stopping":
        return t("state.stopping");
      case "stopped":
        return t("state.stopped");
      case "failed":
      case "unhealthy":
        return t("state.failed");
      case "degraded":
        return t("state.degraded");
      default:
        return value == null || value === "" ? "—" : String(value);
    }
  };
}
