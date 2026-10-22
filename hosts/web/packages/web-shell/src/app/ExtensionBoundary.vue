<script setup lang="ts">
import { MkErrorState, MkSkeleton } from "@mutsuki/ui";
import { toRpcError, useI18n, type RpcError } from "@mutsuki/web-sdk";
import { onErrorCaptured, ref } from "vue";

import { useShell } from "./context.js";

/**
 * Isolates one extension's page: a render, setup or async mount error shows a local error
 * state and is reported to the failures panel; the rest of the shell keeps running.
 */
const props = defineProps<{ extensionId: string }>();
const { reportFailure } = useShell();
const { t } = useI18n();
const error = ref<RpcError | null>(null);
const attempt = ref(0);

onErrorCaptured((failure) => {
  const normalized = toRpcError(failure);
  error.value = normalized;
  reportFailure({ extensionId: props.extensionId, stage: "render", message: normalized.message });
  return false;
});

function retry() {
  error.value = null;
  attempt.value += 1;
}
</script>

<template>
  <MkErrorState
    v-if="error"
    :title="t('shell.boundary.title')"
    :error="{ code: error.code, message: t('shell.boundary.description', { extension: extensionId }) + ' ' + error.message }"
    @retry="retry"
  />
  <Suspense v-else :key="attempt">
    <slot />
    <template #fallback><MkSkeleton /></template>
  </Suspense>
</template>
