<script setup lang="ts">
import { MkButton } from "@mutsuki/ui";
import { useMessages } from "@mutsuki/web-sdk";
import { computed } from "vue";

import { messages } from "../messages";
import type { FieldRendererProps } from "../renderers";
import { emptyValue, itemNode, localized } from "../schema";
import FieldControl from "./FieldControl.vue";

const props = defineProps<FieldRendererProps>();
const emit = defineEmits<{ "update:modelValue": [value: unknown[]] }>();
const { t } = useMessages(messages);
const items = computed(() => (Array.isArray(props.modelValue) ? props.modelValue : []));
const itemType = computed(() => (props.type.kind === "array" ? props.type.item : { kind: "string" as const }));
const item = computed(() => itemNode(props.node, itemType.value, "item"));
const title = computed(() => localized(props.node.title) || props.node.key);

function set(index: number, value: unknown) {
  emit("update:modelValue", items.value.map((current, i) => (i === index ? value : current)));
}
</script>

<template>
  <div class="config-list">
    <div v-for="(value, index) in items" :key="index" class="config-list__row">
      <FieldControl
        :node="item"
        :type="itemType"
        :model-value="value"
        :disabled="disabled"
        :scope="{}"
        :path="[...path, String(index)]"
        :issues="issues"
        :label="t('field.item', { field: title, index: index + 1 })"
        @update:model-value="set(index, $event)"
      />
      <MkButton
        size="sm"
        variant="ghost"
        :disabled="disabled"
        :aria-label="t('field.removeItem', { field: title, index: index + 1 })"
        @click="emit('update:modelValue', items.filter((_, i) => i !== index))"
      >
        {{ t("field.remove") }}
      </MkButton>
    </div>
    <MkButton size="sm" icon="plus" :disabled="disabled" @click="emit('update:modelValue', [...items, emptyValue(item, itemType) ?? ''])">
      {{ t("field.add") }}
    </MkButton>
  </div>
</template>
