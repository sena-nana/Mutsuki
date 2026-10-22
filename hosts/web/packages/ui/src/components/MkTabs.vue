<script setup lang="ts">
import { computed, ref, useId } from "vue";
import type { MkTab } from "../types";

const props = defineProps<{ tabs: MkTab[]; label: string }>();
const active = defineModel<string>({ required: true });
const baseId = useId();
const buttons = ref<HTMLButtonElement[]>([]);
const enabled = computed(() => props.tabs.filter((tab) => !tab.disabled));

const tabId = (id: string) => `${baseId}-tab-${id}`;
const panelId = (id: string) => `${baseId}-panel-${id}`;

function select(id: string, focus = false) {
  active.value = id;
  if (focus) {
    const index = props.tabs.findIndex((tab) => tab.id === id);
    buttons.value[index]?.focus();
  }
}

function onKeydown(event: KeyboardEvent) {
  const list = enabled.value;
  const index = list.findIndex((tab) => tab.id === active.value);
  const target = {
    ArrowRight: list[(index + 1) % list.length],
    ArrowLeft: list[(index - 1 + list.length) % list.length],
    Home: list[0],
    End: list[list.length - 1],
  }[event.key];
  if (!target) return;
  event.preventDefault();
  select(target.id, true);
}
</script>

<template>
  <div class="mk-tabs">
    <div class="mk-tabs__list" role="tablist" :aria-label="label" @keydown="onKeydown">
      <button
        v-for="tab in tabs"
        :id="tabId(tab.id)"
        :key="tab.id"
        ref="buttons"
        type="button"
        role="tab"
        class="mk-tabs__tab mk-focusable"
        :aria-selected="tab.id === active ? 'true' : 'false'"
        :aria-controls="panelId(tab.id)"
        :tabindex="tab.id === active ? 0 : -1"
        :disabled="tab.disabled"
        @click="select(tab.id)"
      >
        {{ tab.label }}
      </button>
    </div>
    <div
      v-for="tab in tabs"
      v-show="tab.id === active"
      :id="panelId(tab.id)"
      :key="tab.id"
      role="tabpanel"
      class="mk-tabs__panel"
      :aria-labelledby="tabId(tab.id)"
      tabindex="0"
    >
      <slot v-if="tab.id === active" :name="tab.id" />
    </div>
  </div>
</template>

<style>
.mk-tabs__list {
  display: flex;
  gap: var(--mk-space-1);
  border-bottom: 1px solid var(--mk-color-border);
  overflow-x: auto;
}
.mk-tabs__tab {
  position: relative;
  padding: var(--mk-space-2) var(--mk-space-3);
  border: 0;
  background: transparent;
  color: var(--mk-color-text-muted);
  font: inherit;
  cursor: pointer;
  white-space: nowrap;
}
.mk-tabs__tab:hover:not(:disabled) {
  color: var(--mk-color-text);
}
.mk-tabs__tab[aria-selected="true"] {
  color: var(--mk-color-text);
  font-weight: 600;
}
.mk-tabs__tab[aria-selected="true"]::after {
  content: "";
  position: absolute;
  inset: auto var(--mk-space-2) -1px;
  height: 2px;
  border-radius: 2px;
  background: var(--mk-color-accent);
}
.mk-tabs__tab:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.mk-tabs__panel {
  padding-top: var(--mk-space-4);
}
.mk-tabs__panel:focus-visible {
  outline: 2px solid var(--mk-focus-ring);
  outline-offset: 2px;
}
</style>
