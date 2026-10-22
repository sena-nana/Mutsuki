<script setup lang="ts">
import { ref } from "vue";

const props = defineProps<{ label: string }>();
const count = ref(0);
</script>

<template>
  <button class="fixture-panel" type="button" @click="count += 1">
    {{ props.label }} {{ count }}
  </button>
</template>

<style>
.fixture-panel {
  color: var(--mk-color-text);
}
</style>
