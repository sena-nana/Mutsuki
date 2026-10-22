<script setup lang="ts">
import { useFieldControl } from "../field";

/** Empty input is `null`, never `0` or `NaN`. */
defineProps<{ min?: number; max?: number; step?: number; disabled?: boolean; label?: string }>();
const value = defineModel<number | null>({ default: null });
const control = useFieldControl();

function onInput(event: Event) {
  const raw = (event.target as HTMLInputElement).value;
  const parsed = raw === "" ? null : Number(raw);
  value.value = parsed === null || Number.isNaN(parsed) ? null : parsed;
}
</script>

<template>
  <input
    :id="control.id.value"
    class="mk-input"
    type="number"
    inputmode="decimal"
    :value="value ?? ''"
    :min="min"
    :max="max"
    :step="step"
    :disabled="disabled"
    :aria-label="label"
    :aria-describedby="control.describedBy.value"
    :aria-invalid="control.invalid.value ? 'true' : undefined"
    :required="control.required.value"
    @input="onInput"
  />
</template>
