<script setup lang="ts">
import { computed, ref } from "vue";
import { useUiText } from "../messages";

/** Two columns with a keyboard-resizable divider; stacks below the compact breakpoint. */
const props = withDefaults(defineProps<{ min?: number; max?: number; label?: string }>(), {
  min: 15,
  max: 85,
});
const size = defineModel<number>({ default: 35 });
const root = ref<HTMLElement | null>(null);
const text = useUiText();
const style = computed(() => ({ "--mk-split-start": `${size.value}%` }));

function clamp(value: number) {
  return Math.min(props.max, Math.max(props.min, Math.round(value)));
}

function onKeydown(event: KeyboardEvent) {
  const step = event.shiftKey ? 10 : 2;
  const next = {
    ArrowLeft: size.value - step,
    ArrowRight: size.value + step,
    Home: props.min,
    End: props.max,
  }[event.key];
  if (next === undefined) return;
  event.preventDefault();
  size.value = clamp(next);
}

function onPointerDown(event: PointerEvent) {
  const host = root.value;
  if (!host) return;
  const target = event.currentTarget as HTMLElement;
  target.setPointerCapture?.(event.pointerId);
  const move = (moveEvent: PointerEvent) => {
    const rect = host.getBoundingClientRect();
    if (rect.width > 0) size.value = clamp(((moveEvent.clientX - rect.left) / rect.width) * 100);
  };
  const up = () => {
    target.removeEventListener("pointermove", move);
    target.removeEventListener("pointerup", up);
  };
  target.addEventListener("pointermove", move);
  target.addEventListener("pointerup", up);
}
</script>

<template>
  <div class="mk-split mk-container">
    <div ref="root" class="mk-split__grid" :style="style">
      <div class="mk-split__pane"><slot name="start" /></div>
      <div
        class="mk-split__divider mk-focusable"
        role="separator"
        aria-orientation="vertical"
        :aria-label="label ?? text('resize')"
        :aria-valuenow="size"
        :aria-valuemin="min"
        :aria-valuemax="max"
        tabindex="0"
        @keydown="onKeydown"
        @pointerdown="onPointerDown"
      />
      <div class="mk-split__pane"><slot name="end" /></div>
    </div>
  </div>
</template>

<style>
.mk-split__grid {
  display: grid;
  grid-template-columns: var(--mk-split-start) 6px minmax(0, 1fr);
  min-height: 0;
  height: 100%;
}
.mk-split__pane {
  min-width: 0;
  min-height: 0;
  overflow: auto;
}
.mk-split__divider {
  cursor: col-resize;
  background: linear-gradient(var(--mk-color-border), var(--mk-color-border)) center / 1px 100% no-repeat;
  touch-action: none;
}
.mk-split__divider:hover {
  background-size: 3px 100%;
}
@container (max-width: 640px) {
  .mk-split__grid {
    grid-template-columns: minmax(0, 1fr);
  }
  .mk-split__divider {
    display: none;
  }
}
</style>
