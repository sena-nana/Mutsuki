<script setup lang="ts">
import type { SlotRegistration } from "@mutsuki/web-sdk";
import { computed, markRaw, type Component } from "vue";

/** One `overview.cards` contribution on the homepage grid; the host owns the cell size. */
const props = defineProps<{ item: SlotRegistration }>();
const span = computed(() => ({
  "--overview-card-col-span": String(clamp(props.item.colSpan, 2, 4)),
  "--overview-card-row-span": String(clamp(props.item.rowSpan, 1, 8)),
}));
const component = computed(() => markRaw(props.item.component as Component));

function clamp(value: number | undefined, fallback: number, max: number): number {
  return Number.isInteger(value) && value! >= 1 ? Math.min(max, value!) : fallback;
}
</script>

<template>
  <article class="card card--outlined" :style="span">
    <component :is="component" />
  </article>
</template>
