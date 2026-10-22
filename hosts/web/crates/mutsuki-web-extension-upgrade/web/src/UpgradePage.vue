<script setup lang="ts">
import { MkAsync, MkButton, MkCard, MkCodeBlock, MkErrorState, MkSearchInput } from "@mutsuki/ui";
import { callRpc, toRpcError, useMessages, useMutsukiRuntime, useRpc, type RpcError } from "@mutsuki/web-sdk";
import { reactive, ref } from "vue";

import { upgradeApi, type UpgradeModule } from "./contract";
import { messages } from "./messages";

const { t } = useMessages(messages);
const { client } = useMutsukiRuntime();
const filter = ref("");
const applied = ref("");
const report = useRpc(upgradeApi.check, () => ({ query: applied.value }));
const plans = reactive(new Map<string, { pending: boolean; text?: string; error?: RpcError }>());

function search(value: string) {
  applied.value = value.trim();
}

function check() {
  if (applied.value === filter.value.trim()) void report.refresh();
  else search(filter.value);
}

async function plan(module: UpgradeModule) {
  plans.set(module.id, { pending: true });
  try {
    const value = await callRpc(client, upgradeApi.plan, { module_id: module.id });
    plans.set(module.id, {
      pending: false,
      text: value.cli_command || JSON.stringify(value.plan, null, 2),
    });
  } catch (error) {
    plans.set(module.id, { pending: false, error: toRpcError(error) });
  }
}

const revision = (module: UpgradeModule) =>
  t("revision", { from: module.current_revision || "—", to: module.remote_revision || "—" });
</script>

<template>
  <div class="page-body upgrade-page">
    <div class="toolbar row-item">
      <MkSearchInput v-model="filter" :label="t('filter')" :placeholder="t('filter')" @search="search" />
      <MkButton icon="refresh" :loading="report.loading.value" @click="check">{{ t("check") }}</MkButton>
    </div>
    <MkAsync
      :source="report"
      :is-empty="(value) => !value.modules?.length"
      :empty-title="t('none')"
    >
      <template #default="{ data }">
        <MkCard v-for="module in data.modules" :key="module.id" :title="module.id">
          <p class="muted">{{ revision(module) }}</p>
          <MkButton :loading="plans.get(module.id)?.pending" @click="plan(module)">{{ t("plan") }}</MkButton>
          <MkErrorState
            v-if="plans.get(module.id)?.error"
            :error="plans.get(module.id)!.error!"
            @retry="plan(module)"
          />
          <MkCodeBlock
            v-else-if="plans.get(module.id)?.text"
            :code="plans.get(module.id)!.text!"
            language="shell"
          />
        </MkCard>
      </template>
    </MkAsync>
  </div>
</template>
