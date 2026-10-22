<script setup lang="ts">
/**
 * Visually hidden announcement region (connection state, background results). Mount it
 * once and change `message`; screen readers announce each change.
 */
withDefaults(defineProps<{ message: string; politeness?: "polite" | "assertive" }>(), {
  politeness: "polite",
});
</script>

<template>
  <div
    class="mk-visually-hidden"
    :role="politeness === 'assertive' ? 'alert' : 'status'"
    :aria-live="politeness"
    aria-atomic="true"
  >
    {{ message }}
  </div>
</template>
