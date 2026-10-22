<script setup lang="ts" generic="V extends string | number">
import { ref } from "vue";
import type { MkOption } from "../types";

/** Single choice among a few options; arrow keys move and select (radio semantics). */
const props = defineProps<{ label: string; options: MkOption<V>[] }>();
const value = defineModel<V>({ required: true });
const buttons = ref<HTMLButtonElement[]>([]);

function onKeydown(event: KeyboardEvent) {
  const enabled = props.options.filter((option) => !option.disabled);
  const index = enabled.findIndex((option) => option.value === value.value);
  const delta = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[event.key];
  if (delta === undefined || !enabled.length) return;
  event.preventDefault();
  const next = enabled[(index + delta + enabled.length) % enabled.length];
  value.value = next.value;
  buttons.value[props.options.indexOf(next)]?.focus();
}
</script>

<template>
  <div class="mk-segmented" role="radiogroup" :aria-label="label" @keydown="onKeydown">
    <button
      v-for="option in options"
      ref="buttons"
      :key="String(option.value)"
      type="button"
      role="radio"
      class="mk-segmented__option mk-focusable"
      :aria-checked="option.value === value ? 'true' : 'false'"
      :tabindex="option.value === value ? 0 : -1"
      :disabled="option.disabled"
      @click="value = option.value"
    >
      {{ option.label }}
    </button>
  </div>
</template>

<style>
.mk-segmented {
  display: inline-flex;
  gap: 2px;
  padding: 2px;
  border: 1px solid var(--mk-color-border);
  border-radius: var(--mk-radius);
  background: var(--mk-color-subtle);
}
.mk-segmented__option {
  min-height: var(--mk-control-height-sm);
  padding: 0 var(--mk-space-3);
  border: 0;
  border-radius: var(--mk-radius-sm);
  background: transparent;
  color: var(--mk-color-text-muted);
  font: inherit;
  cursor: pointer;
}
.mk-segmented__option:hover:not(:disabled) {
  color: var(--mk-color-text);
}
.mk-segmented__option[aria-checked="true"] {
  background: var(--mk-color-surface-raised);
  color: var(--mk-color-text);
  box-shadow: var(--mk-shadow-menu);
}
</style>
