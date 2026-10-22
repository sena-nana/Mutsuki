<script setup lang="ts">
import { ref, useId } from "vue";
import { useFocusTrap } from "../focus";
import { useUiText } from "../messages";
import MkIconButton from "./MkIconButton.vue";

withDefaults(defineProps<{ title: string; side?: "start" | "end" }>(), {
  side: "end",
});
const open = defineModel<boolean>("open", { required: true });
const panel = ref<HTMLElement | null>(null);
const titleId = useId();
const text = useUiText();
const trap = useFocusTrap(open, panel);

function onKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") {
    event.stopPropagation();
    open.value = false;
    return;
  }
  trap.onKeydown(event);
}
</script>

<template>
  <Teleport to="body">
    <div v-if="open" class="mk-drawer" @keydown="onKeydown">
      <div class="mk-drawer__scrim" aria-hidden="true" @click="open = false" />
      <aside
        ref="panel"
        :class="['mk-drawer__panel', `mk-drawer__panel--${side}`]"
        role="dialog"
        aria-modal="true"
        :aria-labelledby="titleId"
        tabindex="-1"
      >
        <header class="mk-drawer__header">
          <h2 :id="titleId" class="mk-drawer__title">{{ title }}</h2>
          <MkIconButton icon="close" :label="text('close')" @click="open = false" />
        </header>
        <div class="mk-drawer__body"><slot /></div>
        <footer v-if="$slots.footer" class="mk-drawer__footer"><slot name="footer" /></footer>
      </aside>
    </div>
  </Teleport>
</template>

<style>
.mk-drawer {
  position: fixed;
  inset: 0;
  z-index: var(--mk-z-drawer);
}
.mk-drawer__scrim {
  position: absolute;
  inset: 0;
  background: var(--mk-color-scrim);
}
.mk-drawer__panel {
  position: absolute;
  top: 0;
  bottom: 0;
  display: flex;
  flex-direction: column;
  width: min(420px, 100vw);
  background: var(--mk-color-surface-raised);
  color: var(--mk-color-text);
  box-shadow: var(--mk-shadow-dialog);
}
.mk-drawer__panel:focus {
  outline: none;
}
.mk-drawer__panel--end {
  right: 0;
}
.mk-drawer__panel--start {
  left: 0;
}
.mk-drawer__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: var(--mk-space-3) var(--mk-space-4);
  border-bottom: 1px solid var(--mk-color-border-soft);
}
.mk-drawer__title {
  margin: 0;
  font-size: var(--mk-font-size-lg);
}
.mk-drawer__body {
  flex: 1;
  overflow: auto;
  padding: var(--mk-space-4);
}
.mk-drawer__footer {
  display: flex;
  justify-content: flex-end;
  gap: var(--mk-space-2);
  padding: var(--mk-space-3) var(--mk-space-4);
  border-top: 1px solid var(--mk-color-border-soft);
}
</style>
