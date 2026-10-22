<script setup lang="ts">
import { ref } from "vue";
import { useFieldControl } from "../field";
import { useUiText } from "../messages";
import MkIconButton from "./MkIconButton.vue";

/**
 * Write-only secret entry. The stored secret is never sent to the browser: `configured`
 * says one exists, and an empty value means "leave it unchanged".
 */
defineProps<{ configured?: boolean; disabled?: boolean; label?: string }>();
const value = defineModel<string>({ default: "" });
const revealed = ref(false);
const control = useFieldControl();
const text = useUiText();
</script>

<template>
  <div class="mk-input-group">
    <input
      :id="control.id.value"
      v-model="value"
      class="mk-input"
      :type="revealed ? 'text' : 'password'"
      autocomplete="off"
      spellcheck="false"
      :placeholder="configured ? text('secretConfigured') : undefined"
      :disabled="disabled"
      :aria-label="label"
      :aria-describedby="control.describedBy.value"
      :aria-invalid="control.invalid.value ? 'true' : undefined"
    />
    <MkIconButton
      class="mk-input-group__action"
      size="sm"
      :icon="revealed ? 'eye-off' : 'eye'"
      :label="revealed ? text('hide') : text('show')"
      :pressed="revealed"
      :disabled="disabled"
      @click="revealed = !revealed"
    />
  </div>
</template>
