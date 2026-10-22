<script setup lang="ts">
import { nextTick, onBeforeUnmount, ref, useId } from "vue";
import type { MkMenuItem } from "../types";

/**
 * Action menu (WAI-ARIA menu button). The trigger slot receives the attributes and toggle
 * handler to spread on its button; arrows move, Enter selects, Escape returns focus.
 */
defineProps<{ items: MkMenuItem[]; label: string }>();
const emit = defineEmits<{ select: [id: string] }>();
const open = ref(false);
const root = ref<HTMLElement | null>(null);
const menuItems = ref<HTMLButtonElement[]>([]);
const menuId = useId();

function trigger(): HTMLElement | null {
  return root.value?.querySelector<HTMLElement>("[aria-haspopup]") ?? null;
}

function enabledItems() {
  return menuItems.value.filter((item) => !item.disabled);
}

async function show(focus: "first" | "last" = "first") {
  open.value = true;
  document.addEventListener("pointerdown", onOutside, true);
  await nextTick();
  const items = enabledItems();
  (focus === "first" ? items[0] : items[items.length - 1])?.focus();
}

function hide(restoreFocus = true) {
  open.value = false;
  document.removeEventListener("pointerdown", onOutside, true);
  if (restoreFocus) trigger()?.focus();
}

function onOutside(event: PointerEvent) {
  if (!root.value?.contains(event.target as Node)) hide(false);
}

function choose(item: MkMenuItem) {
  if (item.disabled) return;
  emit("select", item.id);
  hide();
}

function onTriggerKeydown(event: KeyboardEvent) {
  if (event.key === "ArrowDown" || event.key === "ArrowUp") {
    event.preventDefault();
    void show(event.key === "ArrowDown" ? "first" : "last");
  }
}

function onMenuKeydown(event: KeyboardEvent) {
  const items = enabledItems();
  const index = items.indexOf(document.activeElement as HTMLButtonElement);
  const target = {
    ArrowDown: items[(index + 1) % items.length],
    ArrowUp: items[(index - 1 + items.length) % items.length],
    Home: items[0],
    End: items[items.length - 1],
  }[event.key];
  if (target) {
    event.preventDefault();
    target.focus();
  } else if (event.key === "Escape") {
    event.preventDefault();
    hide();
  } else if (event.key === "Tab") {
    hide(false);
  }
}

onBeforeUnmount(() => document.removeEventListener("pointerdown", onOutside, true));
</script>

<template>
  <div ref="root" class="mk-menu">
    <slot
      name="trigger"
      :attrs="{
        'aria-haspopup': 'menu',
        'aria-expanded': open ? 'true' : 'false',
        'aria-controls': menuId,
        onKeydown: onTriggerKeydown,
      }"
      :toggle="() => (open ? hide() : show())"
    />
    <div
      v-if="open"
      :id="menuId"
      class="mk-menu__list"
      role="menu"
      :aria-label="label"
      @keydown="onMenuKeydown"
    >
      <button
        v-for="item in items"
        ref="menuItems"
        :key="item.id"
        type="button"
        role="menuitem"
        tabindex="-1"
        :class="['mk-menu__item', { 'mk-menu__item--danger': item.danger }]"
        :disabled="item.disabled"
        @click="choose(item)"
      >
        {{ item.label }}
      </button>
    </div>
  </div>
</template>

<style>
.mk-menu {
  position: relative;
  display: inline-block;
}
.mk-menu__list {
  position: absolute;
  top: calc(100% + var(--mk-space-1));
  right: 0;
  z-index: var(--mk-z-menu);
  display: grid;
  min-width: 160px;
  padding: var(--mk-space-1);
  border: 1px solid var(--mk-color-border);
  border-radius: var(--mk-radius);
  background: var(--mk-color-surface-raised);
  box-shadow: var(--mk-shadow-menu);
}
.mk-menu__item {
  padding: var(--mk-space-2) var(--mk-space-3);
  border: 0;
  border-radius: var(--mk-radius-sm);
  background: transparent;
  color: var(--mk-color-text);
  font: inherit;
  text-align: start;
  cursor: pointer;
}
.mk-menu__item:hover:not(:disabled),
.mk-menu__item:focus-visible {
  background: var(--mk-state-hover);
  outline: none;
}
.mk-menu__item--danger {
  color: var(--mk-color-err);
}
.mk-menu__item:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
</style>
