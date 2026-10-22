<script setup lang="ts">
import { computed } from "vue";

import type { ConfigNode, ConfigValueType, Draft, ValidationIssue } from "../schema";
import { useFieldRenderers } from "../renderers";

/** The bare control for one value: a format renderer if registered, else the kind's built-in. */
const props = defineProps<{
  node: ConfigNode;
  type?: ConfigValueType;
  modelValue: unknown;
  disabled: boolean;
  scope: Draft;
  path: string[];
  issues: Map<string, ValidationIssue[]>;
  label?: string;
}>();
const emit = defineEmits<{ "update:modelValue": [value: unknown] }>();
const renderers = useFieldRenderers();
const type = computed(() => props.type ?? props.node.value_type);
const renderer = computed(() => renderers.format(props.node.presentation?.format) ?? renderers.kind(type.value.kind));
</script>

<template>
  <component
    :is="renderer"
    v-if="renderer"
    :node="node"
    :type="type"
    :model-value="modelValue"
    :disabled="disabled"
    :scope="scope"
    :path="path"
    :issues="issues"
    :label="label"
    @update:model-value="emit('update:modelValue', $event)"
  />
</template>
