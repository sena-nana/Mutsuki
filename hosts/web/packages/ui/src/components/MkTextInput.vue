<script setup lang="ts">
import { useFieldControl } from "../field";

withDefaults(
  defineProps<{
    type?: "text" | "email" | "url" | "password";
    placeholder?: string;
    disabled?: boolean;
    autocomplete?: string;
    /** Accessible name when the input is not inside an `MkField`. */
    label?: string;
  }>(),
  { type: "text" },
);
const value = defineModel<string>({ default: "" });
const control = useFieldControl();
</script>

<template>
  <input
    :id="control.id.value"
    v-model="value"
    class="mk-input"
    :type="type"
    :placeholder="placeholder"
    :disabled="disabled"
    :autocomplete="autocomplete"
    :aria-label="label"
    :aria-describedby="control.describedBy.value"
    :aria-invalid="control.invalid.value ? 'true' : undefined"
    :required="control.required.value"
  />
</template>
