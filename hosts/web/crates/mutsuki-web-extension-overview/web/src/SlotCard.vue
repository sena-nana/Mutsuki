<script setup lang="ts">
import type { Disposable, SlotRegistration } from "@mutsuki/web-sdk";
import { computed, markRaw, onBeforeUnmount, onMounted, ref, type Component } from "vue";

/** One `overview.cards` contribution: a Vue component, or a legacy `{ mount }` object. */
const props = defineProps<{ item: SlotRegistration }>();
const host = ref<HTMLElement | null>(null);
let mounted: Disposable | { destroy?(): void } | void | null = null;

const legacy = computed(() => {
  const component = props.item.component as Record<string, unknown> | null;
  return !!component && typeof component.mount === "function" && !("setup" in component) && !("render" in component);
});
const span = computed(() => ({
  "--overview-card-col-span": String(clamp(props.item.colSpan, 2, 4)),
  "--overview-card-row-span": String(clamp(props.item.rowSpan, 1, 8)),
}));

function clamp(value: number | undefined, fallback: number, max: number): number {
  return Number.isInteger(value) && value! >= 1 ? Math.min(max, value!) : fallback;
}

onMounted(async () => {
  if (!legacy.value || !host.value) return;
  const component = props.item.component as { mount(element: HTMLElement): unknown };
  mounted = (await component.mount(host.value)) as typeof mounted;
});
onBeforeUnmount(() => {
  const owned = mounted as { dispose?(): void; destroy?(): void } | null;
  owned?.dispose ? owned.dispose() : owned?.destroy?.();
});
const vueComponent = computed(() => (legacy.value ? null : markRaw(props.item.component as Component)));
</script>

<template>
  <article ref="host" class="card card--outlined" :style="span">
    <component :is="vueComponent" v-if="vueComponent" />
  </article>
</template>
