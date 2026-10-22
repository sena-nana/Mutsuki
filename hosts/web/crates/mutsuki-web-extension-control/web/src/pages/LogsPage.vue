<script setup lang="ts">
import { MkButton, MkCard, MkCodeBlock, MkEmptyState, MkErrorState } from "@mutsuki/ui";
import { callRpc, toRpcError, useMessages, useMutsukiRuntime, usePolling, type RpcError } from "@mutsuki/web-sdk";
import { computed, ref, shallowRef } from "vue";

import { RECONCILE_MS, useDomainChanges } from "../composables";
import { controlApi } from "../contract";
import { messages } from "../messages";

/** Tail window kept in the page. */
const MAX_LINES = 500;

const { t } = useMessages(messages);
const { client } = useMutsukiRuntime();
const entries = shallowRef<Array<{ offset: number; line: string }>>([]);
const error = ref<RpcError | null>(null);
const loading = ref(false);
let cursor: number | null = null;

async function refresh() {
  if (loading.value) return;
  loading.value = true;
  try {
    const result = await callRpc(client, controlApi.log_tail, { cursor, lines: cursor == null ? 50 : MAX_LINES });
    const next = Number(result.cursor ?? 0);
    // A cursor that moved backwards means the log rotated: start a fresh window.
    const base = cursor != null && next < cursor ? [] : entries.value;
    cursor = next;
    const seen = new Set(base.map((entry) => entry.offset));
    entries.value = [...base, ...(result.entries ?? []).filter((entry) => !seen.has(entry.offset))].slice(-MAX_LINES);
    error.value = null;
  } catch (failure) {
    error.value = toRpcError(failure);
  } finally {
    loading.value = false;
  }
}

useDomainChanges(["logs"], () => void refresh());
usePolling(refresh, RECONCILE_MS);
const text = computed(() => entries.value.map((entry) => entry.line).join("\n"));
</script>

<template>
  <div class="page-body">
    <div class="toolbar row-item">
      <MkButton icon="refresh" :loading="loading" @click="refresh()">{{ t("refresh") }}</MkButton>
    </div>
    <MkErrorState v-if="error && !entries.length" :error="error" @retry="refresh()" />
    <MkCard v-else :title="t('logs.title')">
      <p v-if="error" class="err-text">{{ t("refreshFailed") }}</p>
      <MkCodeBlock v-if="entries.length" :code="text" language="log" />
      <MkEmptyState v-else :title="t('logs.empty')" />
    </MkCard>
  </div>
</template>
