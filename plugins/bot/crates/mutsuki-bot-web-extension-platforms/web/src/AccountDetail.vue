<script setup lang="ts">
import { MkButton, MkCard, MkKeyValue, MkStatusBadge, useConfirm, useToast } from "@mutsuki/ui";
import { callRpc, toRpcError, useMessages, useMutsukiRuntime, useSession, type Registry, type SlotRegistration } from "@mutsuki/web-sdk";
import { computed, markRaw, ref, type Component } from "vue";

import { healthTone, renderField } from "./fields";
import { platformsApi } from "./generated/platforms.contract.gen";
import { messages } from "./messages";
import type { PlatformAccountAction, PlatformAccountView, PlatformAdapterView } from "./types";

const props = defineProps<{
  adapter: PlatformAdapterView;
  account: PlatformAccountView;
  slots: Pick<Registry<SlotRegistration>, "list">;
}>();
const emit = defineEmits<{ changed: [] }>();
const { t } = useMessages(messages);
const { client } = useMutsukiRuntime();
const session = useSession();
const confirm = useConfirm();
const toast = useToast();
const running = ref<string | null>(null);

const fields = computed(() =>
  props.adapter.account_fields.map((field) => {
    const rendered = renderField(field.format, props.account.fields[field.key], (key) => t(key as never));
    return { label: field.label, value: rendered.text, mono: rendered.mono, tone: rendered.tone };
  }),
);
const panels = computed(() =>
  props.adapter.detail_panels.flatMap((panel) =>
    props.slots
      .list()
      .filter((item) => item.slot === panel)
      .map((item) => ({ id: item.id, component: markRaw(item.component as Component) })),
  ),
);

async function run(action: PlatformAccountAction) {
  if (
    action.danger &&
    !(await confirm({
      title: action.label,
      message: t("actionConfirm", { account: props.account.display_name, action: action.label }),
      confirmLabel: action.label,
      danger: true,
    }))
  ) {
    return;
  }
  running.value = action.id;
  try {
    const result = await callRpc(client, platformsApi["accounts.action"], {
      adapter_id: props.account.adapter_id,
      account_id: props.account.account_id,
      action: action.id,
    });
    toast.success(result.message || t("actionDone", { action: action.label }));
  } catch (error) {
    toast.error(t("actionFailed", { action: action.label, message: toRpcError(error).message }));
  } finally {
    running.value = null;
    emit("changed");
  }
}
</script>

<template>
  <MkCard :title="account.display_name">
    <template #actions>
      <a
        v-if="adapter.config_provider_id"
        class="mk-button mk-button--ghost mk-button--sm"
        :href="`#/plugins/${adapter.config_provider_id}`"
        >{{ t("configure") }}</a
      >
    </template>
    <p class="row-item">
      <MkStatusBadge :tone="healthTone(account.health)" :label="t(`health.${account.health}`)" />
      <span class="muted">{{ adapter.label }} · {{ account.account_id }} · {{ account.connection_state }}</span>
      <MkStatusBadge v-if="!account.enabled" tone="neutral" :label="t('disabled')" />
    </p>
    <p v-if="account.last_error" class="error-banner" role="alert">
      {{ t("lastError") }}：{{ account.last_error.message }}（{{ account.last_error.code }}）
    </p>
    <MkKeyValue :items="fields" />
    <div v-if="adapter.account_actions.length" class="toolbar row-item" role="group" :aria-label="t('actions')">
      <MkButton
        v-for="action in adapter.account_actions"
        :key="action.id"
        size="sm"
        :variant="action.danger ? 'danger' : 'secondary'"
        :disabled="!session.can(action.capability) || (running !== null && running !== action.id)"
        :loading="running === action.id"
        @click="run(action)"
      >
        {{ action.label }}
      </MkButton>
    </div>
  </MkCard>
  <component :is="panel.component" v-for="panel in panels" :key="panel.id" :adapter="adapter" :account="account" />
</template>
