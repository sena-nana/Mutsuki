<script setup lang="ts">
import { computed } from "vue";
import { ICON_PATHS, type IconName } from "./icons";

const props = withDefaults(defineProps<{ name: IconName; label?: string; size?: number }>(), {
  size: 18,
});
const path = computed(() => ICON_PATHS[props.name]);
</script>

<template>
  <svg
    class="mk-icon"
    viewBox="0 0 24 24"
    :width="size"
    :height="size"
    fill="none"
    stroke="currentColor"
    stroke-width="1.7"
    stroke-linecap="round"
    stroke-linejoin="round"
    :role="label ? 'img' : undefined"
    :aria-label="label"
    :aria-hidden="label ? undefined : 'true'"
    focusable="false"
  >
    <path :d="path" />
  </svg>
</template>

<style>
.mk-icon {
  flex: none;
  vertical-align: middle;
}
</style>
