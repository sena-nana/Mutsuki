<script setup lang="ts">
import { MkTextInput } from "@mutsuki/ui";
import { useMessages } from "@mutsuki/web-sdk";

import { messages } from "../messages";
import type { FieldRendererProps } from "../renderers";

/** `cron-expression` format: one line, monospace, with the field order as placeholder. */
defineProps<FieldRendererProps>();
const emit = defineEmits<{ "update:modelValue": [value: string] }>();
const { t } = useMessages(messages);
</script>

<template>
  <MkTextInput
    class="mono"
    :model-value="typeof modelValue === 'string' ? modelValue : ''"
    :placeholder="t('field.cron')"
    :disabled="disabled"
    :label="label"
    @update:model-value="emit('update:modelValue', $event)"
  />
</template>
