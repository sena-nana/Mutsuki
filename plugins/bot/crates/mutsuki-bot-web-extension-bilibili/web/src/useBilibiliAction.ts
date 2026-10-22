import { useToast } from "@mutsuki/ui";
import { callRpc, toRpcError, useMessages, useMutsukiRuntime, type RpcMethodRef } from "@mutsuki/web-sdk";

import { messages } from "./messages";

/** Runs one Bilibili write; failures become a toast and resolve to `undefined`. */
export function useBilibiliAction() {
  const { client } = useMutsukiRuntime();
  const toast = useToast();
  const { t } = useMessages(messages);
  return async <P, R>(method: RpcMethodRef<P, R>, params: P, success?: string): Promise<R | undefined> => {
    try {
      const result = await (callRpc as (c: typeof client, m: RpcMethodRef<P, R>, p: P) => Promise<R>)(client, method, params);
      if (success) toast.success(success);
      return result;
    } catch (error) {
      toast.error(t("actionFailed", { message: toRpcError(error).message }));
      return undefined;
    }
  };
}
