<script setup lang="ts">
import { useFieldControl } from "../field";

defineProps<{ disabled?: boolean }>();
const checked = defineModel<boolean>({ default: false });
const control = useFieldControl();
</script>

<template>
  <label class="mk-check">
    <input
      :id="control.id.value"
      v-model="checked"
      type="checkbox"
      class="mk-check__input"
      :disabled="disabled"
      :aria-describedby="control.describedBy.value"
    />
    <span class="mk-check__label"><slot /></span>
  </label>
</template>

<style>
.mk-check {
  display: inline-flex;
  align-items: center;
  gap: var(--mk-space-2);
  cursor: pointer;
}
.mk-check__input {
  width: 16px;
  height: 16px;
  margin: 0;
  accent-color: var(--mk-color-accent);
}
.mk-check__input:focus-visible {
  outline: 2px solid var(--mk-focus-ring);
  outline-offset: 2px;
}
</style>
