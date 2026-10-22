<script setup lang="ts">
import { ref, useId } from "vue";
import { useFocusTrap } from "../focus";
import { useUiText } from "../messages";
import MkIconButton from "./MkIconButton.vue";

/**
 * Modal dialog. `alert` makes it an alertdialog (confirmations); `persistent` ignores Escape
 * and scrim clicks so an in-flight action cannot be dismissed by accident.
 */
withDefaults(
  defineProps<{
    title: string;
    description?: string;
    alert?: boolean;
    persistent?: boolean;
    size?: "sm" | "md" | "lg";
  }>(),
  { size: "md" },
);
const open = defineModel<boolean>("open", { required: true });
const emit = defineEmits<{ dismiss: [] }>();
const panel = ref<HTMLElement | null>(null);
const titleId = useId();
const descriptionId = useId();
const text = useUiText();
const trap = useFocusTrap(open, panel);

function dismiss(persistent: boolean) {
  if (persistent) return;
  open.value = false;
  emit("dismiss");
}

function onKeydown(event: KeyboardEvent, persistent: boolean) {
  if (event.key === "Escape") {
    event.stopPropagation();
    dismiss(persistent);
    return;
  }
  trap.onKeydown(event);
}
</script>

<template>
  <Teleport to="body">
    <div v-if="open" class="mk-dialog" @keydown="onKeydown($event, !!persistent)">
      <div class="mk-dialog__scrim" aria-hidden="true" @click="dismiss(!!persistent)" />
      <div
        ref="panel"
        :class="['mk-dialog__panel', `mk-dialog__panel--${size}`]"
        :role="alert ? 'alertdialog' : 'dialog'"
        aria-modal="true"
        :aria-labelledby="titleId"
        :aria-describedby="description ? descriptionId : undefined"
        tabindex="-1"
      >
        <header class="mk-dialog__header">
          <h2 :id="titleId" class="mk-dialog__title">{{ title }}</h2>
          <MkIconButton
            v-if="!persistent"
            icon="close"
            size="sm"
            :label="text('close')"
            @click="dismiss(false)"
          />
        </header>
        <p v-if="description" :id="descriptionId" class="mk-dialog__description">
          {{ description }}
        </p>
        <div class="mk-dialog__body"><slot /></div>
        <footer v-if="$slots.footer" class="mk-dialog__footer"><slot name="footer" /></footer>
      </div>
    </div>
  </Teleport>
</template>

<style>
.mk-dialog {
  position: fixed;
  inset: 0;
  z-index: var(--mk-z-dialog);
  display: grid;
  place-items: center;
  padding: var(--mk-space-4);
}
.mk-dialog__scrim {
  position: absolute;
  inset: 0;
  background: var(--mk-color-scrim);
}
.mk-dialog__panel {
  position: relative;
  display: flex;
  flex-direction: column;
  width: 100%;
  max-height: calc(100vh - 2 * var(--mk-space-4));
  border: 1px solid var(--mk-color-border);
  border-radius: var(--mk-radius-lg);
  background: var(--mk-color-surface-raised);
  color: var(--mk-color-text);
  box-shadow: var(--mk-shadow-dialog);
}
.mk-dialog__panel:focus {
  outline: none;
}
.mk-dialog__panel--sm {
  max-width: 380px;
}
.mk-dialog__panel--md {
  max-width: 520px;
}
.mk-dialog__panel--lg {
  max-width: 760px;
}
.mk-dialog__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--mk-space-3);
  padding: var(--mk-space-4) var(--mk-space-4) 0;
}
.mk-dialog__title {
  margin: 0;
  font-size: var(--mk-font-size-lg);
}
.mk-dialog__description {
  margin: var(--mk-space-2) var(--mk-space-4) 0;
  color: var(--mk-color-text-muted);
}
.mk-dialog__body {
  overflow: auto;
  padding: var(--mk-space-3) var(--mk-space-4);
}
.mk-dialog__footer {
  display: flex;
  justify-content: flex-end;
  gap: var(--mk-space-2);
  padding: var(--mk-space-3) var(--mk-space-4) var(--mk-space-4);
}
</style>
