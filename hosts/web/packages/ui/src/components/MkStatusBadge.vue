<script setup lang="ts">
/** Status as text plus tone; color is never the only signal. */
import type { MkTone } from "../types";

withDefaults(defineProps<{ tone?: MkTone; label: string }>(), {
  tone: "neutral",
});
</script>

<template>
  <span :class="['mk-badge', `mk-badge--${tone}`]">
    <span class="mk-badge__dot" aria-hidden="true" />{{ label }}
  </span>
</template>

<style>
.mk-badge {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 2px var(--mk-space-2);
  border-radius: var(--mk-radius-pill);
  background: var(--mk-color-subtle);
  color: var(--mk-color-text);
  font-size: var(--mk-font-size-sm);
  line-height: 18px;
  white-space: nowrap;
}
.mk-badge__dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--mk-color-text-faint);
}
.mk-badge--ok {
  background: var(--mk-color-ok-soft);
}
.mk-badge--ok .mk-badge__dot {
  background: var(--mk-color-ok);
}
.mk-badge--warn {
  background: var(--mk-color-warn-soft);
}
.mk-badge--warn .mk-badge__dot {
  background: var(--mk-color-warn);
}
.mk-badge--err {
  background: var(--mk-color-err-soft);
}
.mk-badge--err .mk-badge__dot {
  background: var(--mk-color-err);
}
.mk-badge--info {
  background: var(--mk-color-accent-soft);
}
.mk-badge--info .mk-badge__dot {
  background: var(--mk-color-accent);
}
</style>
