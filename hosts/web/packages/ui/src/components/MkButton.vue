<script setup lang="ts">
import MkIcon from "./MkIcon.vue";
import type { IconName } from "./icons";

withDefaults(
  defineProps<{
    variant?: "primary" | "secondary" | "ghost" | "danger";
    size?: "sm" | "md";
    type?: "button" | "submit" | "reset";
    icon?: IconName;
    loading?: boolean;
    disabled?: boolean;
    block?: boolean;
  }>(),
  { variant: "secondary", size: "md", type: "button" },
);
</script>

<template>
  <button
    :type="type"
    :class="[
      'mk-button',
      `mk-button--${variant}`,
      `mk-button--${size}`,
      { 'mk-button--block': block, 'is-loading': loading },
    ]"
    :disabled="disabled || loading"
    :aria-busy="loading ? 'true' : undefined"
  >
    <span v-if="loading" class="mk-button__spinner" aria-hidden="true" />
    <MkIcon v-else-if="icon" :name="icon" :size="16" />
    <span class="mk-button__label"><slot /></span>
  </button>
</template>

<style>
.mk-button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: var(--mk-space-2);
  height: var(--mk-control-height);
  padding: 0 var(--mk-space-3);
  border: 1px solid var(--mk-color-border);
  border-radius: var(--mk-radius);
  background: var(--mk-color-surface-raised);
  color: var(--mk-color-text);
  font: inherit;
  font-size: var(--mk-font-size);
  line-height: 1;
  white-space: nowrap;
  cursor: pointer;
  transition: background-color 120ms ease, border-color 120ms ease;
}
.mk-button:hover:not(:disabled) {
  background: var(--mk-state-hover);
}
.mk-button:active:not(:disabled) {
  background: var(--mk-state-pressed);
}
.mk-button:disabled {
  opacity: 0.55;
  cursor: not-allowed;
}
.mk-button--sm {
  height: var(--mk-control-height-sm);
  padding: 0 var(--mk-space-2);
  font-size: var(--mk-font-size-sm);
}
.mk-button--block {
  display: flex;
  width: 100%;
}
.mk-button--primary {
  border-color: transparent;
  background: var(--mk-color-accent-solid);
  color: var(--mk-color-on-accent);
}
.mk-button--primary:hover:not(:disabled) {
  background: var(--mk-color-accent-solid-hover);
}
.mk-button--ghost {
  border-color: transparent;
  background: transparent;
}
.mk-button--danger {
  border-color: transparent;
  background: var(--mk-color-danger);
  color: var(--mk-color-on-danger);
}
.mk-button--danger:hover:not(:disabled) {
  background: var(--mk-color-danger-hover);
}
.mk-button__spinner {
  width: 14px;
  height: 14px;
  border: 2px solid currentColor;
  border-right-color: transparent;
  border-radius: 50%;
  animation: mk-spin 0.8s linear infinite;
}
@keyframes mk-spin {
  to {
    transform: rotate(360deg);
  }
}
@media (prefers-reduced-motion: reduce) {
  .mk-button__spinner {
    animation-duration: 2.4s;
  }
}
</style>
