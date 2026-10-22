<script setup lang="ts">
import MkRelativeTime from "./MkRelativeTime.vue";

import type { MkTimelineItem } from "../types";

defineProps<{ items: MkTimelineItem[]; label: string }>();
</script>

<template>
  <ol class="mk-timeline" :aria-label="label">
    <li
      v-for="item in items"
      :key="item.id"
      :class="['mk-timeline__item', `mk-timeline__item--${item.tone ?? 'neutral'}`]"
    >
      <span class="mk-timeline__marker" aria-hidden="true" />
      <div class="mk-timeline__content">
        <slot name="item" :item="item">
          <div class="mk-timeline__head">
            <strong>{{ item.title }}</strong>
            <MkRelativeTime v-if="item.time !== undefined" :value="item.time" />
          </div>
          <p v-if="item.description" class="mk-timeline__description">{{ item.description }}</p>
        </slot>
      </div>
    </li>
  </ol>
</template>

<style>
.mk-timeline {
  margin: 0;
  padding: 0;
  list-style: none;
}
.mk-timeline__item {
  position: relative;
  display: grid;
  grid-template-columns: 16px minmax(0, 1fr);
  gap: var(--mk-space-2);
  padding-bottom: var(--mk-space-3);
}
.mk-timeline__item:not(:last-child)::before {
  content: "";
  position: absolute;
  top: 14px;
  bottom: 0;
  left: 7px;
  width: 1px;
  background: var(--mk-color-border);
}
.mk-timeline__marker {
  width: 9px;
  height: 9px;
  margin: 5px 0 0 3px;
  border-radius: 50%;
  background: var(--mk-color-text-faint);
}
.mk-timeline__item--ok .mk-timeline__marker {
  background: var(--mk-color-ok);
}
.mk-timeline__item--warn .mk-timeline__marker {
  background: var(--mk-color-warn);
}
.mk-timeline__item--err .mk-timeline__marker {
  background: var(--mk-color-err);
}
.mk-timeline__item--info .mk-timeline__marker {
  background: var(--mk-color-accent);
}
.mk-timeline__head {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: var(--mk-space-2);
}
.mk-timeline__head time {
  color: var(--mk-color-text-muted);
  font-size: var(--mk-font-size-sm);
}
.mk-timeline__description {
  margin: var(--mk-space-1) 0 0;
  color: var(--mk-color-text-muted);
}
</style>
