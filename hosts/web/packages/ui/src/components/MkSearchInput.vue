<script setup lang="ts">
import { onBeforeUnmount, watch } from "vue";
import { useFieldControl } from "../field";
import { useUiText } from "../messages";
import MkIconButton from "./MkIconButton.vue";

/** `v-model` updates on every keystroke; `search` fires after `debounceMs` of quiet. */
const props = withDefaults(defineProps<{ label?: string; placeholder?: string; debounceMs?: number }>(), {
  debounceMs: 250,
});
const emit = defineEmits<{ search: [query: string] }>();
const value = defineModel<string>({ default: "" });
const control = useFieldControl();
const text = useUiText();
let timer: ReturnType<typeof setTimeout> | undefined;

watch(value, (query) => {
  clearTimeout(timer);
  timer = setTimeout(() => emit("search", query), props.debounceMs);
});
onBeforeUnmount(() => clearTimeout(timer));

function clear() {
  value.value = "";
  clearTimeout(timer);
  emit("search", "");
}
</script>

<template>
  <div class="mk-input-group" role="search">
    <input
      :id="control.id.value"
      v-model="value"
      class="mk-input"
      type="search"
      :placeholder="placeholder ?? text('search')"
      :aria-label="label ?? text('search')"
      :aria-describedby="control.describedBy.value"
      @keydown.escape="clear"
    />
    <MkIconButton
      v-if="value"
      class="mk-input-group__action"
      size="sm"
      icon="close"
      :label="text('clear')"
      @click="clear"
    />
  </div>
</template>
