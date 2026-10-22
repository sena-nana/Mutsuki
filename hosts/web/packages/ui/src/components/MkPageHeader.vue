<script setup lang="ts">
defineProps<{ title: string; description?: string }>();
</script>

<template>
  <header class="mk-page-header mk-container">
    <div class="mk-page-header__text">
      <h1 class="mk-page-header__title">{{ title }}</h1>
      <p v-if="description" class="mk-page-header__description">{{ description }}</p>
    </div>
    <div v-if="$slots.actions" class="mk-page-header__actions"><slot name="actions" /></div>
  </header>
</template>

<style>
.mk-page-header {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: var(--mk-space-4);
  margin-bottom: var(--mk-space-4);
}
.mk-page-header__title {
  margin: 0;
  font-size: var(--mk-font-size-xl);
  font-weight: 600;
}
.mk-page-header__description {
  margin: var(--mk-space-1) 0 0;
  color: var(--mk-color-text-muted);
}
.mk-page-header__actions {
  display: flex;
  flex-wrap: wrap;
  gap: var(--mk-space-2);
}
@container (max-width: 640px) {
  .mk-page-header {
    flex-direction: column;
    align-items: stretch;
  }
}
</style>
