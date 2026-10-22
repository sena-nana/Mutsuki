<script setup lang="ts">
import { MkTextarea } from "@mutsuki/ui";
import { useMessages } from "@mutsuki/web-sdk";
import { ref, watch } from "vue";

import { messages } from "../messages";
import type { FieldRendererProps } from "../renderers";

/** Untyped value (an object the schema does not describe) edited as JSON text. */
const props = defineProps<FieldRendererProps>();
const emit = defineEmits<{ "update:modelValue": [value: unknown] }>();
const { t } = useMessages(messages);
const format = (value: unknown) => (value === undefined ? "" : JSON.stringify(value, null, 2));
const text = ref(format(props.modelValue));
const error = ref("");

watch(
  () => props.modelValue,
  (value) => {
    if (!error.value) text.value = format(value);
  },
);

function update(next: string) {
  text.value = next;
  if (!next.trim()) {
    error.value = "";
    emit("update:modelValue", undefined);
    return;
  }
  try {
    const parsed: unknown = JSON.parse(next);
    error.value = "";
    emit("update:modelValue", parsed);
  } catch {
    error.value = t("field.invalidJson");
  }
}
</script>

<template>
  <div class="config-json">
    <MkTextarea :model-value="text" :rows="6" :disabled="disabled" :label="label ?? path.join('.')" @update:model-value="update" />
    <p v-if="error" class="mk-field__error" role="alert">{{ error }}</p>
  </div>
</template>
