<script setup lang="ts">
import { MkSecretInput } from "@mutsuki/ui";
import { computed } from "vue";

import type { FieldRendererProps } from "../renderers";
import type { SecretState } from "../schema";

/**
 * Write-only secret. The draft keeps the read state (`configured`/`absent`); typing replaces it
 * with `set`, and clearing the input goes back to "leave unchanged".
 */
const props = defineProps<FieldRendererProps>();
const emit = defineEmits<{ "update:modelValue": [value: SecretState] }>();
const state = computed(() => props.modelValue as SecretState | undefined);
const configured = computed(() => state.value?.state === "configured");
const typed = computed(() => (state.value?.state === "set" ? state.value.value : ""));
const initial = state.value?.state === "set" ? undefined : state.value;

function update(value: string) {
  emit("update:modelValue", value ? { state: "set", value } : (initial ?? { state: "keep" }));
}
</script>

<template>
  <MkSecretInput :model-value="typed" :configured="configured" :disabled="disabled" :label="label" @update:model-value="update" />
</template>
