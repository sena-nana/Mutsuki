<script setup lang="ts">
import { MkAsync, MkButton, MkCodeBlock, MkDataTable, MkDialog, MkSearchInput, MkStatusBadge, useConfirm, type MkTone } from "@mutsuki/ui";
import { formatDateTime, useMessages, useRpc } from "@mutsuki/web-sdk";
import { computed, ref } from "vue";

import { PAGE_SIZE, qqApi, qqChanged, type DeliveryStatus, type QqDelivery } from "./contract";
import { messages } from "./messages";
import { useQqWrite } from "./useQqWrite";

const { t } = useMessages(messages);
const confirm = useConfirm();
const write = useQqWrite();
const query = ref("");
const cursors = ref<string[]>([""]);
const pageIndex = ref(0);
const page = useRpc(
  qqApi["deliveries.list"],
  () => ({ query: query.value, after: cursors.value[pageIndex.value] || undefined, limit: PAGE_SIZE }),
  { refreshOn: [qqChanged] },
);
const preview = ref<{ open: boolean; text: string }>({ open: false, text: "" });

const columns = computed(() => [
  { key: "id", label: t("deliveries.id") },
  { key: "status", label: t("deliveries.status") },
  { key: "attempts", label: t("deliveries.attempts"), align: "end" as const },
  { key: "delivered", label: t("deliveries.delivered") },
  { key: "error", label: t("deliveries.error") },
  { key: "actions", label: t("deliveries.actions") },
]);
const rows = computed(() =>
  (page.data.value?.items ?? []).map((item: QqDelivery) => ({
    id: item.receipt.delivery_id,
    status: item.receipt.status,
    attempts: item.receipt.attempt_count,
    delivered: item.receipt.delivered_at_unix_ms ? formatDateTime(item.receipt.delivered_at_unix_ms) : "—",
    error: item.receipt.error_code ?? "—",
  })),
);

const TONE: Record<DeliveryStatus, MkTone> = {
  pending: "neutral",
  sending: "info",
  succeeded: "ok",
  retry_scheduled: "warn",
  permanently_failed: "err",
  cancelled: "neutral",
  previewed: "info",
  reconcile_required: "err",
};
const retryable = (status: DeliveryStatus) => ["retry_scheduled", "permanently_failed", "reconcile_required"].includes(status);
const cancellable = (status: DeliveryStatus) => ["pending", "retry_scheduled", "reconcile_required"].includes(status);

function search(value: string) {
  query.value = value.trim();
  cursors.value = [""];
  pageIndex.value = 0;
}

async function showPreview(id: string) {
  const result = await write({ action: "delivery_preview", delivery_id: id });
  if (result !== undefined) preview.value = { open: true, text: JSON.stringify(result, null, 2) };
}

async function retry(id: string) {
  if (!(await confirm({ title: t("deliveries.retry"), message: t("deliveries.retryConfirm", { id }) }))) return;
  await write({ action: "delivery_retry", delivery_id: id });
  await page.refresh();
}

async function cancel(id: string) {
  if (!(await confirm({ title: t("deliveries.cancel"), message: t("deliveries.cancelConfirm", { id }), danger: true }))) return;
  await write({ action: "delivery_cancel", delivery_id: id });
  await page.refresh();
}

function next() {
  const cursor = page.data.value?.next_cursor;
  if (!cursor) return;
  cursors.value[pageIndex.value + 1] = cursor;
  pageIndex.value += 1;
}
</script>

<template>
  <div class="page-body">
    <div class="toolbar row-item">
      <MkSearchInput :label="t('search')" @search="search" />
      <MkButton icon="refresh" :loading="page.loading.value" @click="page.refresh()">{{ t("refresh") }}</MkButton>
    </div>
    <MkAsync :source="page" :is-empty="(value) => !value.items.length" :empty-title="t('deliveries.empty')">
      <MkDataTable :caption="t('deliveries.caption')" row-key="id" :columns="columns" :rows="rows">
        <template #cell-id="{ value }"><span class="mono">{{ value }}</span></template>
        <template #cell-status="{ value }">
          <MkStatusBadge :tone="TONE[value as DeliveryStatus]" :label="t(`status.${value}` as never)" />
        </template>
        <template #cell-actions="{ row }">
          <div class="row-actions">
            <MkButton size="sm" variant="ghost" @click="showPreview(String(row.id))">{{ t("deliveries.preview") }}</MkButton>
            <MkButton v-if="retryable(row.status as DeliveryStatus)" size="sm" @click="retry(String(row.id))">
              {{ t("deliveries.retry") }}
            </MkButton>
            <MkButton v-if="cancellable(row.status as DeliveryStatus)" size="sm" variant="danger" @click="cancel(String(row.id))">
              {{ t("deliveries.cancel") }}
            </MkButton>
          </div>
        </template>
      </MkDataTable>
    </MkAsync>
    <div class="toolbar row-item">
      <MkButton size="sm" :disabled="pageIndex === 0" @click="pageIndex -= 1">{{ t("previous") }}</MkButton>
      <MkButton size="sm" :disabled="!page.data.value?.next_cursor" @click="next">{{ t("next") }}</MkButton>
    </div>
    <MkDialog v-model:open="preview.open" :title="t('deliveries.previewTitle')" size="lg">
      <MkCodeBlock :code="preview.text" language="json" />
    </MkDialog>
  </div>
</template>
