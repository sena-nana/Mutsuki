<script setup lang="ts">
import { MkButton, MkTextInput } from "@mutsuki/ui";
import { useMessages } from "@mutsuki/web-sdk";
import { computed } from "vue";

import { messages } from "../messages";
import type { FieldRendererProps } from "../renderers";
import { emptyValue, itemNode, localized } from "../schema";
import FieldControl from "./FieldControl.vue";

/** String-keyed entries; renaming a key keeps its value and position. */
const props = defineProps<FieldRendererProps>();
const emit = defineEmits<{ "update:modelValue": [value: Record<string, unknown>] }>();
const { t } = useMessages(messages);
const entries = computed(() =>
  Object.entries(props.modelValue && typeof props.modelValue === "object" ? (props.modelValue as Record<string, unknown>) : {}),
);
const valueType = computed(() => (props.type.kind === "map" ? props.type.value : { kind: "string" as const }));
const entryNode = computed(() => itemNode(props.node, valueType.value, "value"));
const title = computed(() => localized(props.node.title) || props.node.key);

const commit = (next: Array<[string, unknown]>) => emit("update:modelValue", Object.fromEntries(next));

function rename(index: number, key: string) {
  const trimmed = key.trim();
  if (!trimmed || entries.value.some(([existing], i) => i !== index && existing === trimmed)) return;
  commit(entries.value.map((entry, i) => (i === index ? [trimmed, entry[1]] : entry)));
}

function add() {
  let index = entries.value.length + 1;
  while (entries.value.some(([key]) => key === `key${index}`)) index += 1;
  commit([...entries.value, [`key${index}`, emptyValue(entryNode.value, valueType.value) ?? ""]]);
}
</script>

<template>
  <div class="config-list">
    <div v-for="([key, value], index) in entries" :key="index" class="config-list__row">
      <MkTextInput
        :model-value="key"
        :disabled="disabled"
        :label="t('field.mapKey', { field: title, index: index + 1 })"
        @update:model-value="rename(index, $event)"
      />
      <FieldControl
        :node="entryNode"
        :type="valueType"
        :model-value="value"
        :disabled="disabled"
        :scope="{}"
        :path="[...path, key]"
        :issues="issues"
        :label="t('field.mapValue', { field: title, key })"
        @update:model-value="commit(entries.map((entry, i) => (i === index ? [key, $event] : entry)))"
      />
      <MkButton size="sm" variant="ghost" :disabled="disabled" @click="commit(entries.filter((_, i) => i !== index))">
        {{ t("field.remove") }}
      </MkButton>
    </div>
    <MkButton size="sm" icon="plus" :disabled="disabled" @click="add">{{ t("field.addEntry") }}</MkButton>
  </div>
</template>
