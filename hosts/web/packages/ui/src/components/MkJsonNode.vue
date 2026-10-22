<script setup lang="ts">
import { computed } from "vue";

defineOptions({ name: "MkJsonNode" });
const props = defineProps<{ name?: string; value: unknown; depth: number; openDepth: number }>();

const kind = computed(() =>
  props.value === null ? "null" : Array.isArray(props.value) ? "array" : typeof props.value,
);
const entries = computed(() =>
  kind.value === "array"
    ? (props.value as unknown[]).map((item, index) => [String(index), item] as const)
    : kind.value === "object"
      ? Object.entries(props.value as Record<string, unknown>)
      : [],
);
const summary = computed(() =>
  kind.value === "array" ? `[${entries.value.length}]` : `{${entries.value.length}}`,
);
const scalar = computed(() =>
  kind.value === "string" ? JSON.stringify(props.value) : String(props.value),
);
</script>

<template>
  <details
    v-if="kind === 'array' || kind === 'object'"
    class="mk-json__node"
    :open="depth < openDepth"
  >
    <summary class="mk-focusable">
      <span v-if="name !== undefined" class="mk-json__key">{{ name }}:</span>
      <span class="mk-json__summary">{{ summary }}</span>
    </summary>
    <div class="mk-json__children">
      <MkJsonNode
        v-for="[key, child] in entries"
        :key="key"
        :name="key"
        :value="child"
        :depth="depth + 1"
        :open-depth="openDepth"
      />
    </div>
  </details>
  <div v-else class="mk-json__leaf">
    <span v-if="name !== undefined" class="mk-json__key">{{ name }}:</span>
    <span :class="['mk-json__value', `mk-json__value--${kind}`]">{{ scalar }}</span>
  </div>
</template>
