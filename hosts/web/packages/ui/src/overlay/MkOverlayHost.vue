<script setup lang="ts">
import { computed, inject, ref, watch } from "vue";
import MkButton from "../components/MkButton.vue";
import MkDialog from "../components/MkDialog.vue";
import MkField from "../components/MkField.vue";
import MkIconButton from "../components/MkIconButton.vue";
import MkTextInput from "../components/MkTextInput.vue";
import { useUiText } from "../messages";
import { OVERLAY_STORE } from "./store";

/** Renders the oldest pending confirm/prompt and the toast stack. Mount once per app. */
const store = inject(OVERLAY_STORE);
if (!store) throw new Error("MkOverlayHost requires installOverlays(app)");
const text = useUiText();
const current = computed(() => store.requests[0] ?? null);
const open = computed({
  get: () => current.value !== null,
  set: (value) => {
    if (!value && current.value) store.settle(current.value.id, null);
  },
});
const typed = ref("");
const promptValue = ref("");
const promptError = ref<string | null>(null);

watch(current, (request) => {
  typed.value = "";
  promptError.value = null;
  promptValue.value = request?.kind === "prompt" ? (request.options.initial ?? "") : "";
});

const confirmBlocked = computed(() => {
  const request = current.value;
  return request?.kind === "confirm" && !!request.options.requireText
    ? typed.value !== request.options.requireText
    : false;
});

function accept() {
  const request = current.value;
  if (!request) return;
  if (request.kind === "confirm") {
    if (!confirmBlocked.value) store!.settle(request.id, true);
    return;
  }
  const error = request.options.validate?.(promptValue.value) ?? null;
  promptError.value = error;
  if (!error) store!.settle(request.id, promptValue.value);
}

function cancel() {
  if (current.value) store!.settle(current.value.id, current.value.kind === "confirm" ? false : null);
}
</script>

<template>
  <MkDialog
    v-if="current"
    :key="current.id"
    v-model:open="open"
    :title="current.options.title"
    :description="current.options.message"
    :alert="current.kind === 'confirm' && !!current.options.danger"
    size="sm"
  >
    <form id="mk-overlay-form" class="mk-overlay__form" @submit.prevent="accept">
      <MkField
        v-if="current.kind === 'confirm' && current.options.requireText"
        :label="text('typeToConfirm', { text: current.options.requireText })"
      >
        <MkTextInput v-model="typed" autocomplete="off" />
      </MkField>
      <MkField
        v-if="current.kind === 'prompt'"
        :label="current.options.label"
        :error="promptError"
      >
        <MkTextInput v-model="promptValue" :placeholder="current.options.placeholder" />
      </MkField>
    </form>
    <template #footer>
      <MkButton @click="cancel">{{
        current.kind === "confirm" ? (current.options.cancelLabel ?? text("cancel")) : text("cancel")
      }}</MkButton>
      <MkButton
        type="submit"
        form="mk-overlay-form"
        :variant="current.kind === 'confirm' && current.options.danger ? 'danger' : 'primary'"
        :disabled="confirmBlocked"
      >
        {{ current.options.confirmLabel ?? (current.kind === "confirm" ? text("confirm") : text("ok")) }}
      </MkButton>
    </template>
  </MkDialog>
  <section class="mk-toasts" :aria-label="text('notifications')">
    <div
      v-for="toast in store!.toasts"
      :key="toast.id"
      :class="['mk-toast', `mk-toast--${toast.tone}`]"
      :role="toast.tone === 'err' ? 'alert' : 'status'"
    >
      <span class="mk-toast__message">{{ toast.message }}</span>
      <MkIconButton icon="close" size="sm" :label="text('close')" @click="store!.dismissToast(toast.id)" />
    </div>
  </section>
</template>

<style>
.mk-overlay__form {
  display: grid;
  gap: var(--mk-space-3);
}
.mk-toasts {
  position: fixed;
  right: var(--mk-space-4);
  bottom: var(--mk-space-4);
  z-index: var(--mk-z-toast);
  display: grid;
  gap: var(--mk-space-2);
  width: min(360px, calc(100vw - 2 * var(--mk-space-4)));
}
.mk-toast {
  display: flex;
  align-items: flex-start;
  gap: var(--mk-space-2);
  padding: var(--mk-space-2) var(--mk-space-2) var(--mk-space-2) var(--mk-space-3);
  border: 1px solid var(--mk-color-border);
  border-left-width: 3px;
  border-radius: var(--mk-radius);
  background: var(--mk-color-surface-raised);
  color: var(--mk-color-text);
  box-shadow: var(--mk-shadow-menu);
}
.mk-toast__message {
  flex: 1;
  padding-top: 5px;
}
.mk-toast--ok {
  border-left-color: var(--mk-color-ok);
}
.mk-toast--warn {
  border-left-color: var(--mk-color-warn);
}
.mk-toast--err {
  border-left-color: var(--mk-color-err);
}
.mk-toast--info {
  border-left-color: var(--mk-color-accent);
}
</style>
