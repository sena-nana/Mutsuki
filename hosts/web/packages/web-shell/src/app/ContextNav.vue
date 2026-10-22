<script setup lang="ts">
import { useI18n, type NavigationRegistration } from "@mutsuki/web-sdk";
import { computed } from "vue";

import { groupNavigationItems } from "../runtime.js";
import { useShell } from "./context.js";

const props = defineProps<{ activityId: string; title: string; pageId?: string }>();
const emit = defineEmits<{ navigate: [] }>();
const { model, brand } = useShell();
const { t } = useI18n();
const sections = computed(() =>
  groupNavigationItems(model.navByActivity.get(props.activityId) ?? []),
);

function href(item: NavigationRegistration): string {
  return `#${model.routeFor(props.activityId, item.pageId)}`;
}
</script>

<template>
  <aside class="console-context">
    <div class="console-context__brand">{{ brand }}</div>
    <div class="console-context__title">{{ title }}</div>
    <nav class="console-context__nav" :aria-label="t('shell.pageNavigation')">
      <template v-for="(section, index) in sections" :key="section.group ?? index">
        <div v-if="section.group" class="console-context__group">{{ section.group }}</div>
        <template v-for="item in section.items" :key="item.id">
          <span
            v-if="item.disabled"
            class="sb-tree__row lilia-interactive-item sb-tree__row--muted"
            aria-disabled="true"
            >{{ item.label }}</span
          >
          <a
            v-else
            :href="href(item)"
            :class="['sb-tree__row', 'lilia-interactive-item', { 'is-active': item.pageId === pageId }]"
            :aria-current="item.pageId === pageId ? 'page' : undefined"
            @click="emit('navigate')"
            >{{ item.label }}</a
          >
        </template>
      </template>
    </nav>
  </aside>
</template>
