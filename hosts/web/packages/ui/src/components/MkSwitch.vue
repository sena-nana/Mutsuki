<script setup lang="ts">
import { useFieldControl } from "../field";

defineProps<{ label?: string; disabled?: boolean }>();
const checked = defineModel<boolean>({ default: false });
const control = useFieldControl();
</script>

<template>
  <span class="mk-switch-wrap">
    <button
      :id="control.id.value"
      type="button"
      role="switch"
      class="mk-switch mk-focusable"
      :aria-checked="checked ? 'true' : 'false'"
      :aria-label="label"
      :aria-describedby="control.describedBy.value"
      :disabled="disabled"
      @click="checked = !checked"
    >
      <span class="mk-switch__thumb" aria-hidden="true" />
    </button>
    <slot />
  </span>
</template>

<style>
.mk-switch-wrap {
  display: inline-flex;
  align-items: center;
  gap: var(--mk-space-2);
}
.mk-switch {
  position: relative;
  width: 36px;
  height: 20px;
  padding: 0;
  border: 1px solid var(--mk-color-border-strong);
  border-radius: var(--mk-radius-pill);
  background: var(--mk-color-subtle);
  cursor: pointer;
}
.mk-switch__thumb {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 14px;
  height: 14px;
  border-radius: 50%;
  background: var(--mk-color-text-muted);
  transition: transform 120ms ease;
}
.mk-switch[aria-checked="true"] {
  border-color: transparent;
  background: var(--mk-color-accent);
}
.mk-switch[aria-checked="true"] .mk-switch__thumb {
  transform: translateX(16px);
  background: var(--mk-color-on-accent);
}
.mk-switch:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
</style>
