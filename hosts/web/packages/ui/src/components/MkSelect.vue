<script setup lang="ts" generic="V extends string | number">
import { useFieldControl } from "../field";
import type { MkOption } from "../types";

/** Native select: platform keyboard and screen-reader support for free. */
defineProps<{ options: MkOption<V>[]; placeholder?: string; disabled?: boolean; label?: string }>();
const value = defineModel<V | null>({ default: null });
const control = useFieldControl();
</script>

<template>
  <select
    :id="control.id.value"
    v-model="value"
    class="mk-input mk-select"
    :disabled="disabled"
    :aria-label="label"
    :aria-describedby="control.describedBy.value"
    :aria-invalid="control.invalid.value ? 'true' : undefined"
    :required="control.required.value"
  >
    <option v-if="placeholder" :value="null" disabled>{{ placeholder }}</option>
    <option
      v-for="option in options"
      :key="String(option.value)"
      :value="option.value"
      :disabled="option.disabled"
    >
      {{ option.label }}
    </option>
  </select>
</template>
