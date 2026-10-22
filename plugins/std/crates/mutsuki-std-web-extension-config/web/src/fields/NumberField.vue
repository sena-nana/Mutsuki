<script setup lang="ts">
import { MkNumberInput } from "@mutsuki/ui";
import { computed } from "vue";

import type { FieldRendererProps } from "../renderers";

const props = defineProps<FieldRendererProps>();
const emit = defineEmits<{ "update:modelValue": [value: number | undefined] }>();
const value = computed(() => (typeof props.modelValue === "number" ? props.modelValue : null));
</script>

<template>
  <MkNumberInput
    :model-value="value"
    :min="node.constraints?.min"
    :max="node.constraints?.max"
    :step="type.kind === 'integer' ? 1 : undefined"
    :disabled="disabled"
    :label="label"
    @update:model-value="emit('update:modelValue', $event === null ? undefined : $event)"
  />
</template>
