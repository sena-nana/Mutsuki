<script setup lang="ts" generic="T">
import { computed, unref } from "vue";
import { useUiText } from "../messages";
import type { MkAsyncSource } from "../types";
import MkEmptyState from "./MkEmptyState.vue";
import MkErrorState from "./MkErrorState.vue";
import MkSkeleton from "./MkSkeleton.vue";

const props = defineProps<{
  source: MkAsyncSource<T>;
  /** Treats the loaded value as "nothing to show". Defaults to null/empty array. */
  isEmpty?: (value: T) => boolean;
  emptyTitle?: string;
}>();
const text = useUiText();
const data = computed(() => unref(props.source.data));
const error = computed(() => unref(props.source.error));
const loading = computed(() => unref(props.source.loading));
const empty = computed(() => {
  const value = data.value;
  if (value === undefined) return false;
  if (props.isEmpty) return props.isEmpty(value);
  return value === null || (Array.isArray(value) && value.length === 0);
});
</script>

<template>
  <div class="mk-async" :aria-busy="loading ? 'true' : 'false'">
    <template v-if="data === undefined">
      <slot v-if="error" name="error" :error="error" :retry="source.refresh">
        <MkErrorState :error="error" @retry="source.refresh?.()" />
      </slot>
      <slot v-else name="loading">
        <span class="mk-visually-hidden" role="status">{{ text("loading") }}</span>
        <MkSkeleton />
      </slot>
    </template>
    <template v-else>
      <slot v-if="error" name="stale" :error="error" />
      <slot v-if="empty" name="empty">
        <MkEmptyState :title="emptyTitle ?? text('empty')" />
      </slot>
      <slot v-else :data="data as T" />
    </template>
  </div>
</template>
