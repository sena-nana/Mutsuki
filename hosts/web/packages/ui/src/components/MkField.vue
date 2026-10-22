<script setup lang="ts">
import { computed, provide, useId } from "vue";
import { FIELD_CONTEXT } from "../field";

const props = defineProps<{ label: string; hint?: string; error?: string | null; required?: boolean }>();
const id = useId();
const hintId = `${id}-hint`;
const errorId = `${id}-error`;
const describedBy = computed(
  () => [props.hint ? hintId : "", props.error ? errorId : ""].filter(Boolean).join(" ") || undefined,
);
provide(FIELD_CONTEXT, {
  id,
  describedBy,
  invalid: computed(() => !!props.error),
  required: computed(() => !!props.required),
});
</script>

<template>
  <div :class="['mk-field', { 'is-invalid': !!error }]">
    <label class="mk-field__label" :for="id">
      {{ label }}<span v-if="required" class="mk-field__required" aria-hidden="true">*</span>
    </label>
    <slot />
    <p v-if="hint" :id="hintId" class="mk-field__hint">{{ hint }}</p>
    <p v-if="error" :id="errorId" class="mk-field__error">{{ error }}</p>
  </div>
</template>

<style>
.mk-field {
  display: grid;
  gap: var(--mk-space-1);
}
.mk-field__label {
  font-weight: 500;
}
.mk-field__required {
  margin-left: 2px;
  color: var(--mk-color-err);
}
.mk-field__hint,
.mk-field__error {
  margin: 0;
  font-size: var(--mk-font-size-sm);
}
.mk-field__hint {
  color: var(--mk-color-text-muted);
}
.mk-field__error {
  color: var(--mk-color-err);
}
.mk-input {
  width: 100%;
  min-height: var(--mk-control-height);
  padding: 0 var(--mk-space-3);
  border: 1px solid var(--mk-color-border);
  border-radius: var(--mk-radius);
  background: var(--mk-color-surface);
  color: var(--mk-color-text);
  font: inherit;
  box-sizing: border-box;
}
.mk-input:hover:not(:disabled) {
  border-color: var(--mk-color-border-strong);
}
.mk-input:focus-visible {
  outline: 2px solid var(--mk-focus-ring);
  outline-offset: -1px;
  border-color: transparent;
}
.mk-input[aria-invalid="true"] {
  border-color: var(--mk-color-err);
}
.mk-input:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}
textarea.mk-input {
  padding: var(--mk-space-2) var(--mk-space-3);
  line-height: 1.5;
  resize: vertical;
}
.mk-input-group {
  position: relative;
  display: flex;
  align-items: center;
}
.mk-input-group .mk-input {
  padding-right: calc(var(--mk-control-height) + var(--mk-space-1));
}
.mk-input-group__action {
  position: absolute;
  right: 2px;
}
</style>
