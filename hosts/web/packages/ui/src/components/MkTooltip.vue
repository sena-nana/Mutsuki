<script setup lang="ts">
import { onBeforeUnmount, ref, useId } from "vue";

/**
 * Supplementary text on hover and keyboard focus. The trigger slot receives `describedby`
 * for its focusable element; Escape hides the tip without moving focus.
 */
withDefaults(defineProps<{ text: string; delayMs?: number }>(), { delayMs: 400 });
const visible = ref(false);
const id = useId();
let timer: ReturnType<typeof setTimeout> | undefined;

function show(delay: number) {
  clearTimeout(timer);
  timer = setTimeout(() => (visible.value = true), delay);
}
function hide() {
  clearTimeout(timer);
  visible.value = false;
}
onBeforeUnmount(() => clearTimeout(timer));
</script>

<template>
  <span
    class="mk-tooltip"
    @mouseenter="show(delayMs)"
    @mouseleave="hide"
    @focusin="show(0)"
    @focusout="hide"
    @keydown.escape="hide"
  >
    <slot :describedby="id" />
    <span :id="id" role="tooltip" :class="['mk-tooltip__bubble', { 'is-visible': visible }]">
      {{ text }}
    </span>
  </span>
</template>

<style>
.mk-tooltip {
  position: relative;
  display: inline-flex;
}
.mk-tooltip__bubble {
  position: absolute;
  bottom: calc(100% + 6px);
  left: 50%;
  z-index: var(--mk-z-menu);
  max-width: 280px;
  padding: var(--mk-space-1) var(--mk-space-2);
  border-radius: var(--mk-radius-sm);
  background: var(--mk-color-text);
  color: var(--mk-color-bg);
  font-size: var(--mk-font-size-sm);
  white-space: normal;
  width: max-content;
  transform: translateX(-50%);
  pointer-events: none;
  opacity: 0;
  visibility: hidden;
  transition: opacity 100ms ease;
}
.mk-tooltip__bubble.is-visible {
  opacity: 1;
  visibility: visible;
}
</style>
