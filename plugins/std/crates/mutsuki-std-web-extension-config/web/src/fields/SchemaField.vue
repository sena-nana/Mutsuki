<script setup lang="ts">
import { MkField } from "@mutsuki/ui";
import { useMessages } from "@mutsuki/web-sdk";
import { computed, useId } from "vue";

import { messages } from "../messages";
import { isComposite, useFieldRenderers } from "../renderers";
import { isEditable, isVisible, localized, nodeTitle, pathKey, type ConfigNode, type Draft, type ValidationIssue } from "../schema";
import FieldControl from "./FieldControl.vue";

/** One labelled field: title, description, restart hint and its validation issues. */
const props = defineProps<{
  node: ConfigNode;
  scope: Draft;
  path: string[];
  issues: Map<string, ValidationIssue[]>;
  disabled?: boolean;
}>();
const emit = defineEmits<{ update: [key: string, value: unknown] }>();
const { t } = useMessages(messages);
const renderers = useFieldRenderers();
const legendId = useId();

const visible = computed(() => isVisible(props.node, props.scope));
const disabled = computed(() => !!props.disabled || !isEditable(props.node, props.scope));
const composite = computed(() => isComposite(props.node, props.node.value_type, renderers));
const error = computed(
  () => props.issues.get(pathKey(props.path))?.map((issue) => localized(issue.message) || issue.code).join("；") || null,
);
const hint = computed(() => {
  const policy = props.node.restart_policy;
  const restart = policy && policy !== "none" ? t(`restart.${policy}`) : "";
  return [localized(props.node.description), restart].filter(Boolean).join(" ") || undefined;
});
</script>

<template>
  <template v-if="visible">
    <fieldset v-if="composite" class="config-fieldset" :aria-describedby="hint ? `${legendId}-hint` : undefined">
      <legend class="config-fieldset__legend">
        {{ nodeTitle(node) }}<span v-if="node.constraints?.required" class="mk-field__required" aria-hidden="true">*</span>
      </legend>
      <p v-if="hint" :id="`${legendId}-hint`" class="mk-field__hint">{{ hint }}</p>
      <p v-if="error" class="mk-field__error" role="alert">{{ error }}</p>
      <FieldControl
        :node="node"
        :model-value="scope[node.key]"
        :disabled="disabled"
        :scope="scope"
        :path="path"
        :issues="issues"
        @update:model-value="emit('update', node.key, $event)"
      />
    </fieldset>
    <MkField v-else :label="nodeTitle(node)" :hint="hint" :error="error" :required="node.constraints?.required">
      <FieldControl
        :node="node"
        :model-value="scope[node.key]"
        :disabled="disabled"
        :scope="scope"
        :path="path"
        :issues="issues"
        @update:model-value="emit('update', node.key, $event)"
      />
    </MkField>
  </template>
</template>
