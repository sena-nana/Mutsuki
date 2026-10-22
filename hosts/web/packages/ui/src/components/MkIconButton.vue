<script setup lang="ts">
import MkIcon from "./MkIcon.vue";
import type { IconName } from "./icons";

/** Icon-only button. `label` is required: it is the accessible name and the tooltip. */
withDefaults(
  defineProps<{
    icon: IconName;
    label: string;
    size?: "sm" | "md";
    pressed?: boolean;
    disabled?: boolean;
    type?: "button" | "submit";
  }>(),
  { size: "md", type: "button", pressed: undefined },
);
</script>

<template>
  <button
    :type="type"
    :class="['mk-icon-button', `mk-icon-button--${size}`]"
    :aria-label="label"
    :title="label"
    :aria-pressed="pressed"
    :disabled="disabled"
  >
    <MkIcon :name="icon" :size="size === 'sm' ? 14 : 18" />
  </button>
</template>

<style>
.mk-icon-button {
  display: inline-grid;
  place-items: center;
  width: var(--mk-control-height);
  height: var(--mk-control-height);
  padding: 0;
  border: 0;
  border-radius: var(--mk-radius);
  background: transparent;
  color: var(--mk-color-text-muted);
  cursor: pointer;
}
.mk-icon-button:hover:not(:disabled) {
  background: var(--mk-state-hover);
  color: var(--mk-color-text);
}
.mk-icon-button[aria-pressed="true"] {
  background: var(--mk-state-selected);
  color: var(--mk-color-text);
}
.mk-icon-button:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.mk-icon-button--sm {
  width: var(--mk-control-height-sm);
  height: var(--mk-control-height-sm);
}
</style>
