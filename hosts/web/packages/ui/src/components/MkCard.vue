<script setup lang="ts">
import { useId } from "vue";

withDefaults(defineProps<{ title?: string; variant?: "outlined" | "flat"; as?: string }>(), {
  variant: "outlined",
  as: "section",
});
const titleId = useId();
</script>

<template>
  <component
    :is="as"
    :class="['mk-card', `mk-card--${variant}`]"
    :aria-labelledby="title || $slots.title ? titleId : undefined"
  >
    <header v-if="title || $slots.title || $slots.actions" class="mk-card__header">
      <h2 :id="titleId" class="mk-card__title"><slot name="title">{{ title }}</slot></h2>
      <div v-if="$slots.actions" class="mk-card__actions"><slot name="actions" /></div>
    </header>
    <div class="mk-card__body"><slot /></div>
    <footer v-if="$slots.footer" class="mk-card__footer"><slot name="footer" /></footer>
  </component>
</template>

<style>
.mk-card {
  display: flex;
  flex-direction: column;
  min-width: 0;
  border-radius: var(--mk-radius-lg);
  background: var(--mk-color-surface);
  color: var(--mk-color-text);
}
.mk-card--outlined {
  border: 1px solid var(--mk-color-border);
}
.mk-card__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--mk-space-3);
  padding: var(--mk-space-3) var(--mk-space-4) 0;
}
.mk-card__title {
  margin: 0;
  font-size: var(--mk-font-size-lg);
  font-weight: 600;
}
.mk-card__actions {
  display: flex;
  gap: var(--mk-space-2);
}
.mk-card__body {
  padding: var(--mk-space-3) var(--mk-space-4) var(--mk-space-4);
  min-width: 0;
}
.mk-card__footer {
  display: flex;
  justify-content: flex-end;
  gap: var(--mk-space-2);
  padding: var(--mk-space-3) var(--mk-space-4);
  border-top: 1px solid var(--mk-color-border-soft);
}
</style>
