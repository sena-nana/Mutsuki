<script setup lang="ts" generic="V extends string | number">
import { useId } from "vue";
import type { MkOption } from "../types";

defineProps<{ legend: string; options: MkOption<V>[]; disabled?: boolean }>();
const value = defineModel<V | null>({ default: null });
const name = useId();
</script>

<template>
  <fieldset class="mk-radio-group" :disabled="disabled">
    <legend class="mk-radio-group__legend">{{ legend }}</legend>
    <label v-for="option in options" :key="String(option.value)" class="mk-check">
      <input
        v-model="value"
        type="radio"
        class="mk-check__input"
        :name="name"
        :value="option.value"
        :disabled="option.disabled"
      />
      <span>{{ option.label }}</span>
    </label>
  </fieldset>
</template>

<style>
.mk-radio-group {
  display: grid;
  gap: var(--mk-space-2);
  margin: 0;
  padding: 0;
  border: 0;
}
.mk-radio-group__legend {
  margin-bottom: var(--mk-space-1);
  padding: 0;
  font-weight: 500;
}
</style>
