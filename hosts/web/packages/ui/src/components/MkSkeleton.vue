<script setup lang="ts">
/** Placeholder shapes only; the surrounding `MkAsync` announces loading. */
withDefaults(defineProps<{ lines?: number; variant?: "text" | "block" }>(), {
  lines: 3,
  variant: "text",
});
</script>

<template>
  <div :class="['mk-skeleton', `mk-skeleton--${variant}`]" aria-hidden="true">
    <template v-if="variant === 'text'">
      <span v-for="line in lines" :key="line" class="mk-skeleton__line" />
    </template>
    <span v-else class="mk-skeleton__block" />
  </div>
</template>

<style>
.mk-skeleton {
  display: grid;
  gap: var(--mk-space-2);
}
.mk-skeleton__line,
.mk-skeleton__block {
  display: block;
  border-radius: var(--mk-radius-sm);
  background: var(--mk-color-subtle);
  animation: mk-pulse 1.4s ease-in-out infinite;
}
.mk-skeleton__line {
  height: 12px;
}
.mk-skeleton__line:last-child {
  width: 60%;
}
.mk-skeleton__block {
  height: 96px;
}
@keyframes mk-pulse {
  50% {
    opacity: 0.45;
  }
}
@media (prefers-reduced-motion: reduce) {
  .mk-skeleton__line,
  .mk-skeleton__block {
    animation: none;
  }
}
</style>
