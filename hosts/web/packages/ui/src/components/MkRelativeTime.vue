<script setup lang="ts">
import { formatDateTime, formatRelativeTime } from "@mutsuki/web-sdk";
import { computed, onBeforeUnmount, ref } from "vue";

/** Relative time that stays current; the absolute time is the title and `datetime`. */
const props = withDefaults(
  defineProps<{ value: Date | number | string | null | undefined; refreshMs?: number }>(),
  { refreshMs: 30_000 },
);
const now = ref(Date.now());
const timer = setInterval(() => (now.value = Date.now()), props.refreshMs);
onBeforeUnmount(() => clearInterval(timer));

const date = computed(() => {
  if (props.value == null || props.value === "") return null;
  const parsed = props.value instanceof Date ? props.value : new Date(props.value);
  return Number.isNaN(parsed.getTime()) ? null : parsed;
});
</script>

<template>
  <time
    v-if="date"
    class="mk-relative-time"
    :datetime="date.toISOString()"
    :title="formatDateTime(date)"
    >{{ formatRelativeTime(date, now) }}</time
  >
  <span v-else class="mk-relative-time">—</span>
</template>
