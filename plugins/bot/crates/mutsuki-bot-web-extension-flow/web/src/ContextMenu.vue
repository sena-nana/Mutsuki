<script setup lang="ts">
import { useMessages } from "@mutsuki/web-sdk";
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from "vue";

import { filterMenu, type MenuEntry } from "./flow-model";
import { messages } from "./messages";

export interface MenuItem extends MenuEntry {
  danger?: boolean;
}

/**
 * Fixed-position menu at the pointer, clamped to the viewport. The searchable form filters by
 * label and keywords; arrows move, Enter selects, Escape or an outside press closes.
 */
const props = defineProps<{ x: number; y: number; items: MenuItem[]; searchable?: boolean; label: string }>();
const emit = defineEmits<{ select: [item: MenuItem]; close: [] }>();
const { t } = useMessages(messages);
const query = ref("");
const active = ref(0);
const root = ref<HTMLElement | null>(null);
const search = ref<HTMLInputElement | null>(null);

const visible = computed(() => filterMenu(props.items, query.value) as MenuItem[]);
const choices = computed(() => visible.value.filter((item) => !item.header));
const style = computed(() => ({
  left: `${Math.max(8, Math.min(props.x, globalThis.innerWidth - 280))}px`,
  top: `${Math.max(8, Math.min(props.y, globalThis.innerHeight - 360))}px`,
}));

function choose(item: MenuItem | undefined) {
  if (item && !item.header) emit("select", item);
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") {
    event.preventDefault();
    emit("close");
  } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
    event.preventDefault();
    const count = choices.value.length;
    if (count) active.value = (active.value + (event.key === "ArrowDown" ? 1 : count - 1)) % count;
    void nextTick(() => root.value?.querySelector<HTMLElement>(".ctx-menu__item--active")?.scrollIntoView?.({ block: "nearest" }));
  } else if (event.key === "Enter") {
    event.preventDefault();
    choose(choices.value[active.value]);
  }
}

const outside = (event: PointerEvent) => {
  if (!root.value?.contains(event.target as Node)) emit("close");
};

onMounted(() => {
  document.addEventListener("pointerdown", outside, true);
  if (props.searchable) search.value?.focus();
  else root.value?.querySelector<HTMLElement>("button")?.focus();
});
onBeforeUnmount(() => document.removeEventListener("pointerdown", outside, true));
</script>

<template>
  <Teleport to="body">
    <div
      ref="root"
      :class="['ctx-menu', { 'ctx-menu--searchable': searchable }]"
      role="menu"
      :aria-label="label"
      :style="style"
      @keydown="onKeydown"
    >
      <div v-if="searchable" class="ctx-menu__search">
        <input ref="search" v-model="query" type="search" :placeholder="t('menu.search')" :aria-label="t('menu.search')" @input="active = 0" />
      </div>
      <div class="ctx-menu__scroll">
        <template v-for="item in visible" :key="item.id">
          <div v-if="item.header" class="ctx-menu__header" role="presentation">{{ item.label }}</div>
          <button
            v-else
            type="button"
            role="menuitem"
            :class="['ctx-menu__item', { 'ctx-menu__item--danger': item.danger, 'ctx-menu__item--active': choices[active]?.id === item.id }]"
            @click="choose(item)"
            @pointerenter="active = choices.indexOf(item)"
          >
            <span class="ctx-menu__label">{{ item.label }}</span>
          </button>
        </template>
        <p v-if="!choices.length" class="ctx-menu__empty">{{ t("menu.empty") }}</p>
      </div>
    </div>
  </Teleport>
</template>
