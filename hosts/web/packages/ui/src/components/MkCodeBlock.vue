<script setup lang="ts">
import MkCopyButton from "./MkCopyButton.vue";

withDefaults(defineProps<{ code: string; language?: string; copyable?: boolean }>(), {
  copyable: true,
});
</script>

<template>
  <figure class="mk-code">
    <figcaption v-if="language || copyable" class="mk-code__bar">
      <span class="mk-code__language">{{ language }}</span>
      <MkCopyButton v-if="copyable" :text="code" />
    </figcaption>
    <pre class="mk-code__pre" tabindex="0"><code>{{ code }}</code></pre>
  </figure>
</template>

<style>
.mk-code {
  margin: 0;
  border: 1px solid var(--mk-color-border);
  border-radius: var(--mk-radius);
  background: var(--mk-color-subtle);
}
.mk-code__bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: var(--mk-space-1) var(--mk-space-2) 0 var(--mk-space-3);
  color: var(--mk-color-text-muted);
  font-size: var(--mk-font-size-sm);
}
.mk-code__pre {
  margin: 0;
  padding: var(--mk-space-2) var(--mk-space-3) var(--mk-space-3);
  overflow: auto;
  font-family: var(--mk-font-mono);
  font-size: var(--mk-font-size-sm);
  line-height: 1.6;
}
.mk-code__pre:focus-visible {
  outline: 2px solid var(--mk-focus-ring);
}
</style>
