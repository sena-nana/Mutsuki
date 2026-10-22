<script setup lang="ts">
import { MkSection } from "@mutsuki/ui";
import { computed } from "vue";

import SchemaField from "./fields/SchemaField.vue";
import { provideFieldRenderers, type FieldRenderers } from "./renderers";
import { groupNodes, isVisible, issuesByPath, type ConfigDescriptor, type Draft, type ValidationResult } from "./schema";

/**
 * Form generated from a `ConfigDescriptor`: declared groups become sections, each field picks a
 * renderer by `presentation.format`, then by value kind. Emits the whole next draft.
 */
const props = defineProps<{
  schema: ConfigDescriptor;
  modelValue: Draft;
  renderers: FieldRenderers;
  validation?: ValidationResult | null;
  disabled?: boolean;
}>();
const emit = defineEmits<{ "update:modelValue": [value: Draft] }>();
provideFieldRenderers(props.renderers);

const groups = computed(() =>
  groupNodes(props.schema)
    .map((group) => ({ ...group, nodes: group.nodes.filter((node) => isVisible(node, props.modelValue)) }))
    .filter((group) => group.nodes.length),
);
const issues = computed(() => issuesByPath(props.validation));

function update(key: string, value: unknown) {
  const next = { ...props.modelValue };
  if (value === undefined) delete next[key];
  else next[key] = value;
  emit("update:modelValue", next);
}
</script>

<template>
  <form class="config-form" novalidate @submit.prevent>
    <template v-for="group in groups" :key="group.id">
      <MkSection v-if="group.title" :title="group.title">
        <SchemaField
          v-for="node in group.nodes"
          :key="node.key"
          :node="node"
          :scope="modelValue"
          :path="[node.key]"
          :issues="issues"
          :disabled="disabled"
          @update="update"
        />
      </MkSection>
      <div v-else class="config-form__fields">
        <SchemaField
          v-for="node in group.nodes"
          :key="node.key"
          :node="node"
          :scope="modelValue"
          :path="[node.key]"
          :issues="issues"
          :disabled="disabled"
          @update="update"
        />
      </div>
    </template>
  </form>
</template>
