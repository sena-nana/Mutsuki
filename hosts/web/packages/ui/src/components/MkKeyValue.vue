<script setup lang="ts">
import type { MkKeyValueItem } from "../types";

defineProps<{ items: MkKeyValueItem[] }>();
</script>

<template>
  <dl class="mk-kv mk-container">
    <div v-for="item in items" :key="item.label" class="mk-kv__row">
      <dt class="mk-kv__label">{{ item.label }}</dt>
      <dd :class="['mk-kv__value', { 'mk-kv__value--mono': item.mono }]">
        <slot :name="item.label" :item="item">{{
          item.value == null || item.value === "" ? "—" : item.value
        }}</slot>
      </dd>
    </div>
  </dl>
</template>

<style>
.mk-kv {
  display: grid;
  gap: var(--mk-space-2);
  margin: 0;
}
.mk-kv__row {
  display: grid;
  grid-template-columns: minmax(96px, 30%) minmax(0, 1fr);
  gap: var(--mk-space-3);
}
.mk-kv__label {
  color: var(--mk-color-text-muted);
}
.mk-kv__value {
  margin: 0;
  overflow-wrap: anywhere;
}
.mk-kv__value--mono {
  font-family: var(--mk-font-mono);
  font-size: var(--mk-font-size-sm);
}
@container (max-width: 640px) {
  .mk-kv__row {
    grid-template-columns: minmax(0, 1fr);
    gap: 0;
  }
}
</style>
