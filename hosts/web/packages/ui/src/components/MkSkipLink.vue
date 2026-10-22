<script setup lang="ts">
import { useUiText } from "../messages";

withDefaults(defineProps<{ target?: string }>(), { target: "#main" });
const text = useUiText();
</script>

<template>
  <a class="mk-skip-link" :href="target"><slot>{{ text("skipToContent") }}</slot></a>
</template>

<style>
.mk-skip-link {
  position: fixed;
  top: var(--mk-space-2);
  left: var(--mk-space-2);
  z-index: var(--mk-z-toast);
  padding: var(--mk-space-2) var(--mk-space-3);
  border-radius: var(--mk-radius);
  background: var(--mk-color-accent);
  color: var(--mk-color-on-accent);
  transform: translateY(-200%);
}
.mk-skip-link:focus {
  transform: none;
  outline: 2px solid var(--mk-focus-ring);
  outline-offset: 2px;
}
</style>
