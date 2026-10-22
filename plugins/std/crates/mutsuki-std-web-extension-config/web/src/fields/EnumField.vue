<script setup lang="ts">
import { MkCheckbox, MkSelect } from "@mutsuki/ui";
import { useMessages } from "@mutsuki/web-sdk";
import { computed } from "vue";

import { messages } from "../messages";
import type { FieldRendererProps } from "../renderers";
import { localized } from "../schema";

/** Single choice as a select (empty = unset), multi choice as a checkbox group. */
const props = defineProps<FieldRendererProps>();
const emit = defineEmits<{ "update:modelValue": [value: string | string[] | undefined] }>();
const { t } = useMessages(messages);
const options = computed(() =>
  props.type.kind === "enum" ? props.type.options.map((option) => ({ value: option.value, label: localized(option.label) || option.value })) : [],
);
const multi = computed(() => props.type.kind === "enum" && !!props.type.multi);
const selected = computed(() => (Array.isArray(props.modelValue) ? props.modelValue.map(String) : []));

function toggle(value: string, on: boolean) {
  const next = new Set(selected.value);
  if (on) next.add(value);
  else next.delete(value);
  emit("update:modelValue", options.value.map((option) => option.value).filter((item) => next.has(item)));
}
</script>

<template>
  <div v-if="multi" class="config-choices">
    <MkCheckbox
      v-for="option in options"
      :key="option.value"
      :model-value="selected.includes(option.value)"
      :disabled="disabled"
      @update:model-value="toggle(option.value, $event)"
    >
      {{ option.label }}
    </MkCheckbox>
  </div>
  <MkSelect
    v-else
    :model-value="typeof modelValue === 'string' ? modelValue : null"
    :options="options"
    :placeholder="t('field.unset')"
    :disabled="disabled"
    :label="label"
    @update:model-value="emit('update:modelValue', $event ?? undefined)"
  />
</template>
