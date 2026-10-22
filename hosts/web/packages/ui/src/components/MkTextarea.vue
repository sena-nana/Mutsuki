<script setup lang="ts">
import { useFieldControl } from "../field";

withDefaults(defineProps<{ rows?: number; placeholder?: string; disabled?: boolean; label?: string }>(), {
  rows: 4,
});
const value = defineModel<string>({ default: "" });
const control = useFieldControl();
</script>

<template>
  <textarea
    :id="control.id.value"
    v-model="value"
    class="mk-input"
    :rows="rows"
    :placeholder="placeholder"
    :disabled="disabled"
    :aria-label="label"
    :aria-describedby="control.describedBy.value"
    :aria-invalid="control.invalid.value ? 'true' : undefined"
    :required="control.required.value"
  />
</template>
