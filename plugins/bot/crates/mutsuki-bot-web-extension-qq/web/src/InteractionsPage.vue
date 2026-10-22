<script setup lang="ts">
import { MkAsync, MkButton, MkDataTable, MkRelativeTime, MkStatusBadge, useConfirm, type MkTone } from "@mutsuki/ui";
import { useMessages, useRpc } from "@mutsuki/web-sdk";
import { computed } from "vue";

import { PAGE_SIZE, qqApi, qqChanged, type InteractionStatus, type QqInteraction } from "./contract";
import { messages } from "./messages";
import { useQqWrite } from "./useQqWrite";

const { t } = useMessages(messages);
const confirm = useConfirm();
const write = useQqWrite();
const page = useRpc(qqApi["interactions.list"], { limit: PAGE_SIZE }, { refreshOn: [qqChanged] });

const columns = computed(() => [
  { key: "id", label: t("interactions.id") },
  { key: "conversation", label: t("interactions.conversation") },
  { key: "status", label: t("interactions.status") },
  { key: "timeout", label: t("interactions.timeout") },
  { key: "retries", label: t("interactions.retries"), align: "end" as const },
  { key: "actions", label: "" },
]);
const where = (item: QqInteraction) =>
  [item.conversation.kind, item.conversation.group_id ?? item.conversation.channel_id ?? item.conversation.user_id]
    .filter(Boolean)
    .join(" · ");
const rows = computed(() =>
  (page.data.value?.items ?? []).map((item) => ({
    id: item.session_id,
    conversation: where(item),
    status: item.status,
    timeout: item.wait.timeout_at_unix_ms,
    retries: item.retries_remaining,
  })),
);
const TONE: Record<InteractionStatus, MkTone> = {
  waiting: "info",
  completed: "ok",
  timed_out: "warn",
  cancelled: "neutral",
  failed: "err",
};

async function cancel(id: string) {
  if (!(await confirm({ title: t("interactions.cancel"), message: t("interactions.cancelConfirm", { id }), danger: true }))) return;
  await write({ action: "interaction_cancel", session_id: id });
  await page.refresh();
}
</script>

<template>
  <div class="page-body">
    <div class="toolbar row-item">
      <MkButton icon="refresh" :loading="page.loading.value" @click="page.refresh()">{{ t("refresh") }}</MkButton>
    </div>
    <MkAsync :source="page" :is-empty="(value) => !value.items.length" :empty-title="t('interactions.empty')">
      <MkDataTable :caption="t('interactions.caption')" row-key="id" :columns="columns" :rows="rows">
        <template #cell-id="{ value }"><span class="mono">{{ value }}</span></template>
        <template #cell-status="{ value }">
          <MkStatusBadge :tone="TONE[value as InteractionStatus]" :label="t(`status.${value}` as never)" />
        </template>
        <template #cell-timeout="{ value }"><MkRelativeTime :value="Number(value)" /></template>
        <template #cell-actions="{ row }">
          <MkButton v-if="row.status === 'waiting'" size="sm" variant="danger" @click="cancel(String(row.id))">
            {{ t("interactions.cancel") }}
          </MkButton>
        </template>
      </MkDataTable>
    </MkAsync>
  </div>
</template>
