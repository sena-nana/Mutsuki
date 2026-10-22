import { useToast } from "@mutsuki/ui";
import { callRpc, toRpcError, useMessages, useMutsukiRuntime, type JsonValue } from "@mutsuki/web-sdk";

import { qqApi, type QqAction } from "./contract";
import { messages } from "./messages";

/**
 * One QQ management write. The owner rejects stale revisions, so each write reads the current
 * revision first; the page has already asked the user, hence `confirmed`.
 */
export function useQqWrite() {
  const { client } = useMutsukiRuntime();
  const toast = useToast();
  const { t } = useMessages(messages);
  return async (action: QqAction): Promise<JsonValue | undefined> => {
    try {
      const { revision } = await callRpc(client, qqApi.snapshot, { query: "" });
      const result = await callRpc(client, qqApi.write, {
        request: { operation_id: "", expected_revision: revision, action },
        confirmed: true,
      });
      return result.result;
    } catch (error) {
      toast.error(t("actionFailed", { message: toRpcError(error).message }));
      return undefined;
    }
  };
}
