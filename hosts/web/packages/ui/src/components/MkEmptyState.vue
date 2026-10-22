<script setup lang="ts">
import MkIcon from "./MkIcon.vue";
import type { IconName } from "./icons";

withDefaults(defineProps<{ title: string; description?: string; icon?: IconName }>(), {
  icon: "info",
});
</script>

<template>
  <div class="mk-state mk-state--empty">
    <MkIcon :name="icon" :size="28" />
    <p class="mk-state__title">{{ title }}</p>
    <p v-if="description" class="mk-state__description">{{ description }}</p>
    <div v-if="$slots.actions" class="mk-state__actions"><slot name="actions" /></div>
  </div>
</template>

<style>
.mk-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--mk-space-2);
  padding: var(--mk-space-6) var(--mk-space-4);
  color: var(--mk-color-text-muted);
  text-align: center;
}
.mk-state__title {
  margin: 0;
  color: var(--mk-color-text);
  font-weight: 600;
}
.mk-state__description {
  margin: 0;
  max-width: 48ch;
}
.mk-state__actions {
  display: flex;
  gap: var(--mk-space-2);
  margin-top: var(--mk-space-2);
}
.mk-state--error .mk-icon {
  color: var(--mk-color-err);
}
.mk-state__code {
  font-family: var(--mk-font-mono);
  font-size: var(--mk-font-size-sm);
}
</style>
