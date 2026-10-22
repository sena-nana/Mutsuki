<script setup lang="ts">
import { MkAsync, MkButton, MkDataTable, useConfirm } from "@mutsuki/ui";
import { callRpc, toRpcError, useMessages, useMutsukiRuntime, usePolling, type RpcError } from "@mutsuki/web-sdk";
import { computed, ref, shallowRef } from "vue";

import { agentApi, type PermissionRequest } from "./contract";
import { messages } from "./messages";
import { useAgentAction } from "./useAgentAction";

/** Approval inbox across sessions: every pending tool permission, newest sessions first. */
const { t } = useMessages(messages);
const { client } = useMutsukiRuntime();
const confirm = useConfirm();
const act = useAgentAction();
const pending = shallowRef<Array<PermissionRequest & { title: string }> | undefined>();
const error = ref<RpcError | null>(null);
const loading = ref(false);

async function refresh() {
  loading.value = true;
  try {
    const page = await callRpc(client, agentApi["sessions.list"], { limit: 50 });
    const waiting = page.items.filter((item) => item.status === "waiting_approval");
    const states = await Promise.all(
      waiting.map((item) => callRpc(client, agentApi["sessions.state"], { session_id: item.session_id })),
    );
    pending.value = states.flatMap((state, index) =>
      state.pending_approvals.map((approval) => ({
        ...approval.request,
        title: waiting[index].title || waiting[index].session_id,
      })),
    );
    error.value = null;
  } catch (failure) {
    error.value = toRpcError(failure);
  } finally {
    loading.value = false;
  }
}
usePolling(refresh, 15_000);

const source = { data: pending, error, loading, refresh };
const columns = computed(() => [
  { key: "title", label: t("approvals.session") },
  { key: "tool", label: t("approvals.tool") },
  { key: "summary", label: t("approvals.summary") },
  { key: "actions", label: "" },
]);

async function decide(request: PermissionRequest, decision: "approved" | "rejected") {
  if (
    decision === "approved" &&
    !(await confirm({ title: t("approvals.approve"), message: t("approvals.approveConfirm", { tool: request.tool, summary: request.summary }) }))
  ) {
    return;
  }
  const method = decision === "approved" ? agentApi["sessions.approve"] : agentApi["sessions.reject"];
  await act(
    method,
    { decision: { session_id: request.session_id, turn_id: request.turn_id, action_id: request.action_id, version: request.version, decision } },
    decision === "approved" ? t("approvals.approved") : t("approvals.rejected"),
  );
  await refresh();
}
</script>

<template>
  <div class="page-body">
    <div class="toolbar row-item">
      <MkButton icon="refresh" :loading="loading" @click="refresh()">{{ t("refresh") }}</MkButton>
    </div>
    <MkAsync :source="source" :empty-title="t('approvals.empty')">
      <template #default="{ data }">
        <MkDataTable :caption="t('approvals.inbox')" row-key="action_id" :columns="columns" :rows="data">
          <template #cell-actions="{ row }">
            <div class="row-actions">
              <MkButton size="sm" variant="primary" @click="decide(row as PermissionRequest, 'approved')">{{ t("approvals.approve") }}</MkButton>
              <MkButton size="sm" @click="decide(row as PermissionRequest, 'rejected')">{{ t("approvals.reject") }}</MkButton>
            </div>
          </template>
        </MkDataTable>
      </template>
    </MkAsync>
  </div>
</template>
