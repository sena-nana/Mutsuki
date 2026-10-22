<script setup lang="ts">
import { MkSwitch } from "@mutsuki/ui";

import type { FieldRendererProps } from "../renderers";

defineProps<FieldRendererProps>();
const emit = defineEmits<{ "update:modelValue": [value: boolean] }>();
</script>

<template>
  <MkSwitch :model-value="!!modelValue" :disabled="disabled" :label="label" @update:model-value="emit('update:modelValue', $event)" />
</template>
