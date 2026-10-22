<script setup lang="ts">
import {
  MkAsync,
  MkButton,
  MkCheckbox,
  MkDataTable,
  MkDialog,
  MkEmptyState,
  MkField,
  MkStatusBadge,
  MkTextInput,
  useConfirm,
} from "@mutsuki/ui";
import { useMessages, useRpc, useSession } from "@mutsuki/web-sdk";
import { computed, reactive, ref } from "vue";

import { NOTIFICATION_KINDS, bilibiliApi, bilibiliChanged, type PreviewCard, type Subscription } from "./contract";
import { formatTarget, safeHttpUrl } from "./format";
import { messages } from "./messages";
import { useBilibiliAction } from "./useBilibiliAction";

const { t } = useMessages(messages);
const session = useSession();
const confirm = useConfirm();
const run = useBilibiliAction();
const status = useRpc(bilibiliApi.status, undefined, { refreshOn: [bilibiliChanged] });
const list = useRpc(bilibiliApi["subscriptions.list"], undefined, { refreshOn: [bilibiliChanged] });
const canWrite = computed(() => session.can("runtime.write") && !!status.data.value?.management_enabled);
const busy = ref<string | null>(null);
const translate = (key: string, params?: Record<string, string | number>) => t(key as never, params);

const columns = computed(() => [
  { key: "subscription_id", label: t("subscriptions.id") },
  { key: "target", label: t("subscriptions.target") },
  { key: "uid", label: t("subscriptions.uid"), align: "end" as const },
  { key: "notifications", label: t("subscriptions.notifications") },
  { key: "outbound_binding", label: t("subscriptions.binding") },
  { key: "paused", label: t("subscriptions.state") },
  { key: "owner", label: t("subscriptions.owner") },
  { key: "actions", label: t("subscriptions.actions") },
]);
const rows = computed(() =>
  (list.data.value?.subscriptions ?? []).map((item) => ({
    ...item,
    target: formatTarget(item.target, translate),
    notifications: item.notifications.map((kind) => t(`notify.${kind}` as never)).join("、"),
    owner: item.owner_user_id ?? "—",
  })),
);

/** Table rows carry display strings; actions work on the subscription they were made from. */
const original = (id: unknown) => list.data.value!.subscriptions.find((item) => item.subscription_id === id)!;

async function refresh() {
  await Promise.all([status.refresh(), list.refresh()]);
}

async function togglePaused(item: Subscription) {
  busy.value = item.subscription_id;
  try {
    await run(bilibiliApi["subscriptions.set_paused"], { selector: item.subscription_id, paused: !item.paused });
    await list.refresh();
  } finally {
    busy.value = null;
  }
}

const preview = ref<{ open: boolean; card: PreviewCard | null }>({ open: false, card: null });
const previewHref = computed(() => safeHttpUrl(preview.value.card?.url));

async function showPreview(item: Subscription) {
  busy.value = item.subscription_id;
  try {
    const card = await run(bilibiliApi["subscriptions.preview"], { selector: item.subscription_id });
    if (card) preview.value = { open: true, card };
  } finally {
    busy.value = null;
  }
}

async function remove(item: Subscription) {
  const target = formatTarget(item.target, translate);
  const ok = await confirm({
    title: t("subscriptions.delete"),
    message: t("subscriptions.deleteConfirm", { id: item.subscription_id, target }),
    confirmLabel: t("subscriptions.delete"),
    danger: true,
  });
  if (!ok) return;
  await run(
    bilibiliApi["subscriptions.unsubscribe"],
    { subscription_id: item.subscription_id, confirmed: true },
    t("subscriptions.deleted"),
  );
  await refresh();
}

const blank = () => ({
  open: false,
  subscription_id: "",
  uid: "",
  outbound_binding: "",
  group: "",
  notifications: { live: true, dynamic: true, video: true } as Record<(typeof NOTIFICATION_KINDS)[number], boolean>,
  error: "",
});
const editor = reactive(blank());

function openEditor() {
  Object.assign(editor, blank(), { open: true });
}

async function create() {
  const notifications = NOTIFICATION_KINDS.filter((kind) => editor.notifications[kind]);
  const uid = Number(editor.uid);
  if (
    !editor.subscription_id.trim() ||
    !(uid > 0) ||
    !editor.outbound_binding.trim() ||
    !editor.group.trim() ||
    !notifications.length
  ) {
    editor.error = t("subscriptions.required");
    return;
  }
  const created = await run(
    bilibiliApi["subscriptions.subscribe"],
    {
      subscription_id: editor.subscription_id.trim(),
      uid,
      outbound_binding: editor.outbound_binding.trim(),
      notifications,
      target: { type: "group", group_id: editor.group.trim() },
    },
    t("subscriptions.created"),
  );
  if (!created) return;
  editor.open = false;
  await refresh();
}
</script>

<template>
  <div class="page-body">
    <div class="toolbar row-item">
      <MkButton icon="refresh" :loading="list.loading.value" @click="refresh">{{ t("refresh") }}</MkButton>
      <MkButton icon="plus" :disabled="!canWrite" @click="openEditor">{{ t("subscriptions.add") }}</MkButton>
    </div>
    <MkEmptyState
      v-if="status.data.value && !status.data.value.management_enabled"
      :title="t('subscriptions.disabled')"
      :description="status.data.value.reason ?? undefined"
    />
    <MkAsync v-else :source="list" :is-empty="(value) => !value.subscriptions.length" :empty-title="t('subscriptions.empty')">
      <MkDataTable :caption="t('subscriptions.caption')" row-key="subscription_id" :columns="columns" :rows="rows">
        <template #cell-subscription_id="{ value }"><span class="mono">{{ value }}</span></template>
        <template #cell-paused="{ value }">
          <MkStatusBadge
            :tone="value ? 'warn' : 'ok'"
            :label="value ? t('subscriptions.paused') : t('subscriptions.active')"
          />
        </template>
        <template #cell-actions="{ row }">
          <div class="row-actions">
            <MkButton
              size="sm"
              :disabled="!canWrite || busy !== null"
              @click="togglePaused(original(row.subscription_id))"
            >
              {{ row.paused ? t("subscriptions.resume") : t("subscriptions.pause") }}
            </MkButton>
            <MkButton
              size="sm"
              variant="ghost"
              :disabled="busy !== null"
              @click="showPreview(original(row.subscription_id))"
            >
              {{ t("subscriptions.preview") }}
            </MkButton>
            <MkButton
              size="sm"
              variant="danger"
              :disabled="!canWrite || busy !== null"
              @click="remove(original(row.subscription_id))"
            >
              {{ t("subscriptions.delete") }}
            </MkButton>
          </div>
        </template>
      </MkDataTable>
    </MkAsync>
    <MkDialog v-model:open="preview.open" :title="t('subscriptions.previewTitle')">
      <template v-if="preview.card">
        <p><strong>{{ preview.card.title }}</strong></p>
        <p class="muted">{{ preview.card.description }}</p>
        <a v-if="previewHref" :href="previewHref" target="_blank" rel="noreferrer noopener">{{ preview.card.url }}</a>
        <p v-else-if="preview.card.url" class="mono">{{ preview.card.url }}</p>
      </template>
    </MkDialog>
    <MkDialog v-model:open="editor.open" :title="t('subscriptions.add')">
      <form class="mk-overlay__form" @submit.prevent="create">
        <MkField :label="t('subscriptions.id')" required><MkTextInput v-model="editor.subscription_id" /></MkField>
        <MkField :label="t('subscriptions.uid')" required><MkTextInput v-model="editor.uid" /></MkField>
        <MkField :label="t('subscriptions.binding')" required><MkTextInput v-model="editor.outbound_binding" /></MkField>
        <MkField :label="t('subscriptions.group')" required :error="editor.error || null">
          <MkTextInput v-model="editor.group" />
        </MkField>
        <fieldset class="bilibili-notify">
          <legend>{{ t("subscriptions.notifications") }}</legend>
          <MkCheckbox v-for="kind in NOTIFICATION_KINDS" :key="kind" v-model="editor.notifications[kind]">
            {{ t(`notify.${kind}` as never) }}
          </MkCheckbox>
        </fieldset>
      </form>
      <template #footer>
        <MkButton variant="primary" @click="create">{{ t("subscriptions.create") }}</MkButton>
      </template>
    </MkDialog>
  </div>
</template>

<style>
.bilibili-notify {
  display: flex;
  gap: var(--mk-space-4);
  border: 0;
  margin: 0;
  padding: 0;
}
.bilibili-notify legend {
  padding: 0;
  margin-bottom: var(--mk-space-2);
}
</style>
