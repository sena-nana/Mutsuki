<script setup lang="ts">
import { ICON_PATHS, MkIcon, type IconName } from "@mutsuki/ui";
import { useI18n } from "@mutsuki/web-sdk";
import { computed } from "vue";

import type { ConsoleActivity } from "../layout.js";
import { useShell } from "./context.js";

defineProps<{ activeId?: string }>();
const { model } = useShell();
const { t } = useI18n();

const groups = computed(() => ({
  top: model.activities.filter((activity) => activity.position !== "bottom"),
  bottom: model.activities.filter((activity) => activity.position === "bottom"),
}));

function href(activity: ConsoleActivity): string {
  const first = model.navByActivity.get(activity.id)?.find((item) => !item.disabled);
  return first ? `#${model.routeFor(activity.id, first.pageId)}` : "#";
}

function icon(name: string): IconName | null {
  return name in ICON_PATHS ? (name as IconName) : null;
}

</script>

<template>
  <nav class="console-activity" :aria-label="t('shell.workspaces')">
    <template v-for="(list, position) in groups" :key="position">
      <div :class="`console-activity__${position}`">
        <a
          v-for="activity in list"
          :key="activity.id"
          :href="href(activity)"
          :class="['console-activity__item', { 'is-active': activity.id === activeId }]"
          :aria-label="activity.label"
          :title="activity.label"
          :aria-current="activity.id === activeId ? 'page' : undefined"
        >
          <MkIcon v-if="icon(activity.icon)" :name="icon(activity.icon)!" :size="20" />
          <span v-else aria-hidden="true">{{ activity.label.slice(0, 1) }}</span>
        </a>
      </div>
      <div v-if="position === 'top'" class="console-activity__spacer" />
    </template>
  </nav>
</template>
