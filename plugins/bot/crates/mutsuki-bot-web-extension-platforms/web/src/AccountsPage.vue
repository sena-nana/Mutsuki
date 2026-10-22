<script setup lang="ts">
import { MkAsync, MkButton, MkCard, MkEmptyState, MkSplitPane, MkStatusBadge } from "@mutsuki/ui";
import { useMessages, useRpc, type Registry, type SlotRegistration } from "@mutsuki/web-sdk";
import { computed, ref } from "vue";

import AccountDetail from "./AccountDetail.vue";
import { healthTone } from "./fields";
import { changedTopic, platformsApi } from "./generated/platforms.contract.gen";
import { messages } from "./messages";
import type { PlatformAccountView } from "./types";

const props = defineProps<{ slots: Pick<Registry<SlotRegistration>, "list"> }>();
const { t } = useMessages(messages);
const adapters = useRpc(platformsApi["adapters.list"], {}, { refreshOn: [changedTopic] });
const accounts = useRpc(platformsApi["accounts.list"], {}, { refreshOn: [changedTopic] });
const selectedKey = ref<string | null>(null);
const split = ref(32);

const keyOf = (account: PlatformAccountView) => `${account.adapter_id}/${account.account_id}`;
const groups = computed(() =>
  (adapters.data.value ?? []).map((adapter) => ({
    adapter,
    accounts: (accounts.data.value ?? []).filter((account) => account.adapter_id === adapter.adapter_id),
  })),
);
const selected = computed(() => {
  const all = accounts.data.value ?? [];
  return all.find((account) => keyOf(account) === selectedKey.value) ?? all[0] ?? null;
});
const selectedAdapter = computed(() =>
  adapters.data.value?.find((adapter) => adapter.adapter_id === selected.value?.adapter_id) ?? null,
);

function refresh() {
  void adapters.refresh();
  void accounts.refresh();
}
</script>

<template>
  <div class="page-body platforms-page">
    <div class="toolbar row-item">
      <MkButton icon="refresh" :loading="accounts.loading.value" @click="refresh">{{ t("refresh") }}</MkButton>
    </div>
    <MkAsync :source="adapters" :empty-title="t('noAdapters')">
      <template #empty>
        <MkEmptyState :title="t('noAdapters')" :description="t('noAdaptersHint')" />
      </template>
      <MkSplitPane v-model="split" :min="22" :max="50">
        <template #start>
          <MkCard :title="t('accounts')">
            <section v-for="group in groups" :key="group.adapter.adapter_id" class="platforms-group">
              <h3 class="platforms-group__title">{{ group.adapter.label }}</h3>
              <p v-if="!group.accounts.length" class="muted">{{ t("noAccounts") }}</p>
              <button
                v-for="account in group.accounts"
                :key="keyOf(account)"
                type="button"
                class="tree-item mk-focusable platforms-account"
                :aria-pressed="selected && keyOf(selected) === keyOf(account) ? 'true' : 'false'"
                @click="selectedKey = keyOf(account)"
              >
                <strong>{{ account.display_name }}</strong>
                <MkStatusBadge :tone="healthTone(account.health)" :label="t(`health.${account.health}`)" />
              </button>
            </section>
          </MkCard>
        </template>
        <template #end>
          <AccountDetail
            v-if="selected && selectedAdapter"
            :key="keyOf(selected)"
            :adapter="selectedAdapter"
            :account="selected"
            :slots="props.slots"
            @changed="refresh"
          />
          <MkEmptyState v-else :title="t('select')" />
        </template>
      </MkSplitPane>
    </MkAsync>
  </div>
</template>
