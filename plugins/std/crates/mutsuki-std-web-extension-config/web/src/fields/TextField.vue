<script setup lang="ts">
import { MkTextarea, MkTextInput } from "@mutsuki/ui";
import { useMessages } from "@mutsuki/web-sdk";
import { computed } from "vue";

import { messages } from "../messages";
import type { FieldRendererProps } from "../renderers";

/** Strings, file and directory references. */
const props = defineProps<FieldRendererProps>();
const emit = defineEmits<{ "update:modelValue": [value: string] }>();
const { t } = useMessages(messages);
const text = computed(() => (props.modelValue === undefined || props.modelValue === null ? "" : String(props.modelValue)));
const placeholder = computed(
  () =>
    props.node.presentation?.placeholder ||
    (props.type.kind === "file_ref" ? t("field.filePath") : props.type.kind === "directory_ref" ? t("field.directoryPath") : undefined),
);
</script>

<template>
  <MkTextarea
    v-if="type.kind === 'string' && type.multiline"
    :model-value="text"
    :placeholder="placeholder ?? undefined"
    :disabled="disabled"
    :label="label"
    @update:model-value="emit('update:modelValue', $event)"
  />
  <MkTextInput
    v-else
    :model-value="text"
    :placeholder="placeholder ?? undefined"
    :disabled="disabled"
    :label="label"
    @update:model-value="emit('update:modelValue', $event)"
  />
</template>
