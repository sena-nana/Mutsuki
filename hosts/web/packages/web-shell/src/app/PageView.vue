<script setup lang="ts">
import type { PageRegistration } from "@mutsuki/web-sdk";
import { computed, markRaw, type Component } from "vue";

import ExtensionBoundary from "./ExtensionBoundary.vue";
import { useShell } from "./context.js";

const props = defineProps<{ page: PageRegistration }>();
const { runtime } = useShell();
const component = computed<Component>(() => markRaw(props.page.component));
const owner = computed(() => runtime.state.owners.get(props.page.id) ?? "shell");
</script>

<template>
  <ExtensionBoundary :key="page.id" :extension-id="owner">
    <component :is="component" />
  </ExtensionBoundary>
</template>
