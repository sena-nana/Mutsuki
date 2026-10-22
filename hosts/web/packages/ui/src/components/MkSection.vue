<script setup lang="ts">
import { useId } from "vue";

defineProps<{ title: string; description?: string }>();
const titleId = useId();
</script>

<template>
  <section class="mk-section" :aria-labelledby="titleId">
    <div class="mk-section__header">
      <div>
        <h2 :id="titleId" class="mk-section__title">{{ title }}</h2>
        <p v-if="description" class="mk-section__description">{{ description }}</p>
      </div>
      <div v-if="$slots.actions" class="mk-section__actions"><slot name="actions" /></div>
    </div>
    <slot />
  </section>
</template>

<style>
.mk-section {
  display: flex;
  flex-direction: column;
  gap: var(--mk-space-3);
  margin-block: var(--mk-space-5);
}
.mk-section__header {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: var(--mk-space-3);
}
.mk-section__title {
  margin: 0;
  font-size: var(--mk-font-size-lg);
  font-weight: 600;
}
.mk-section__description {
  margin: var(--mk-space-1) 0 0;
  color: var(--mk-color-text-muted);
  font-size: var(--mk-font-size-sm);
}
.mk-section__actions {
  display: flex;
  gap: var(--mk-space-2);
}
</style>
