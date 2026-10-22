<script setup lang="ts">
import { MkButton } from "@mutsuki/ui";
import { useMessages } from "@mutsuki/web-sdk";
import { computed } from "vue";

import { messages } from "../messages";
import type { FieldRendererProps } from "../renderers";
import { emptyValue, sortNodes, type Draft } from "../schema";
import JsonField from "./JsonField.vue";
import SchemaField from "./SchemaField.vue";

/**
 * Nested object. An optional section that is absent stays absent until "add" (so the candidate
 * omits it); objects without declared children are edited as JSON.
 */
const props = defineProps<FieldRendererProps>();
const emit = defineEmits<{ "update:modelValue": [value: unknown] }>();
const { t } = useMessages(messages);
const value = computed(() =>
  props.modelValue && typeof props.modelValue === "object" && !Array.isArray(props.modelValue) ? (props.modelValue as Draft) : null,
);
const children = computed(() => sortNodes(props.node.children ?? []));
const optional = computed(() => !props.node.constraints?.required && props.path.length > 0);

function update(key: string, next: unknown) {
  const object = { ...(value.value ?? {}) };
  if (next === undefined) delete object[key];
  else object[key] = next;
  emit("update:modelValue", object);
}
</script>

<template>
  <JsonField v-if="!children.length" v-bind="props" @update:model-value="emit('update:modelValue', $event)" />
  <div v-else-if="value" class="config-object">
    <SchemaField
      v-for="child in children"
      :key="child.key"
      :node="child"
      :scope="value"
      :path="[...path, child.key]"
      :issues="issues"
      :disabled="disabled"
      @update="update"
    />
    <div v-if="optional" class="toolbar">
      <MkButton size="sm" variant="ghost" :disabled="disabled" @click="emit('update:modelValue', undefined)">
        {{ t("field.removeSection") }}
      </MkButton>
    </div>
  </div>
  <div v-else class="config-object config-object--absent">
    <p class="muted">{{ t("field.sectionAbsent") }}</p>
    <MkButton size="sm" :disabled="disabled" @click="emit('update:modelValue', emptyValue(node) ?? {})">
      {{ t("field.addSection") }}
    </MkButton>
  </div>
</template>
