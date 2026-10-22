<script setup lang="ts">
import { MkButton, MkCard, MkEmptyState, MkErrorState, MkSkeleton, MkSplitPane } from "@mutsuki/ui";
import { callRpc, toRpcError, useMessages, useMutsukiRuntime, type RpcError } from "@mutsuki/web-sdk";
import { onMounted, ref, shallowRef } from "vue";

import { agentApi, type SessionSummary } from "./contract";
import { messages } from "./messages";
import SessionDetail from "./SessionDetail.vue";

const { t } = useMessages(messages);
const { client } = useMutsukiRuntime();
const sessions = shallowRef<SessionSummary[] | null>(null);
const next = ref<string | null>(null);
const error = ref<RpcError | null>(null);
const loading = ref(false);
const selected = ref<string | null>(null);
const split = ref(30);

async function load(after?: string) {
  loading.value = true;
  try {
    const page = await callRpc(client, agentApi["sessions.list"], { limit: 50, ...(after ? { after_session_id: after } : {}) });
    sessions.value = after ? [...(sessions.value ?? []), ...page.items] : page.items;
    next.value = page.next_session_id ?? null;
    error.value = null;
  } catch (failure) {
    error.value = toRpcError(failure);
  } finally {
    loading.value = false;
  }
}

function closed() {
  selected.value = null;
  void load();
}

onMounted(() => void load());
</script>

<template>
  <div class="page-body agent-sessions-page">
    <MkErrorState v-if="error && !sessions" :error="error" @retry="load()" />
    <MkSkeleton v-else-if="!sessions" />
    <MkSplitPane v-else v-model="split" :min="20" :max="45">
      <template #start>
        <MkCard :title="t('sessions.title')">
          <template #actions>
            <MkButton size="sm" icon="refresh" :loading="loading" @click="load()">{{ t("refresh") }}</MkButton>
          </template>
          <p v-if="!sessions.length" class="muted">{{ t("sessions.empty") }}</p>
          <button
            v-for="item in sessions"
            :key="item.session_id"
            type="button"
            class="tree-item row-item mk-focusable agent-session-item"
            :aria-pressed="item.session_id === selected ? 'true' : 'false'"
            @click="selected = item.session_id"
          >
            <span>{{ item.title || item.session_id }}</span>
            <span class="muted">{{ t("sessions.turns", { count: item.turn_count }) }}</span>
          </button>
          <MkButton v-if="next" size="sm" variant="ghost" @click="load(next!)">{{ t("sessions.more") }}</MkButton>
        </MkCard>
      </template>
      <template #end>
        <SessionDetail v-if="selected" :key="selected" :session-id="selected" @closed="closed" />
        <MkEmptyState v-else :title="t('sessions.select')" />
      </template>
    </MkSplitPane>
  </div>
</template>
