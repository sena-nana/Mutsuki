<script setup lang="ts">
import MkJsonNode from "./MkJsonNode.vue";

/** Collapsible JSON tree built on `<details>`, keyboard-operable without custom handlers. */
withDefaults(defineProps<{ value: unknown; openDepth?: number; label?: string }>(), {
  openDepth: 1,
});
</script>

<template>
  <div class="mk-json" role="group" :aria-label="label">
    <MkJsonNode :value="value" :depth="0" :open-depth="openDepth" />
  </div>
</template>

<style>
.mk-json {
  font-family: var(--mk-font-mono);
  font-size: var(--mk-font-size-sm);
  line-height: 1.6;
  overflow-x: auto;
}
.mk-json__node > summary {
  cursor: pointer;
  list-style-position: inside;
}
.mk-json__children {
  padding-left: var(--mk-space-4);
  border-left: 1px solid var(--mk-color-border-soft);
  margin-left: 3px;
}
.mk-json__key {
  margin-right: var(--mk-space-1);
  color: var(--mk-color-text-muted);
}
.mk-json__summary {
  color: var(--mk-color-text-faint);
}
.mk-json__value--string {
  color: var(--mk-color-ok);
}
.mk-json__value--number,
.mk-json__value--boolean {
  color: var(--mk-color-accent);
}
.mk-json__value--null {
  color: var(--mk-color-text-faint);
}
</style>
