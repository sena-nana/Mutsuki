<script setup lang="ts">
import type { PageRegistration } from "@mutsuki/web-sdk";
import { computed, markRaw, type Component } from "vue";

import { isLegacyPage, legacyMount } from "../legacy.js";
import ExtensionBoundary from "./ExtensionBoundary.vue";
import { useShell } from "./context.js";

const props = defineProps<{ page: PageRegistration }>();
const { runtime } = useShell();
const adapted = new WeakMap<object, Component>();

const component = computed<Component>(() => {
  const raw = props.page.component as object;
  if (!isLegacyPage(raw)) return markRaw(raw as Component);
  let wrapped = adapted.get(raw);
  if (!wrapped) {
    wrapped = markRaw(legacyMount(raw));
    adapted.set(raw, wrapped);
  }
  return wrapped;
});
const owner = computed(() => runtime.state.owners.get(props.page.id) ?? "shell");
</script>

<template>
  <ExtensionBoundary :key="page.id" :extension-id="owner">
    <component :is="component" />
  </ExtensionBoundary>
</template>
