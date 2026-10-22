<script setup lang="ts">
import { computed } from "vue";
import { useUiText } from "../messages";
import MkButton from "./MkButton.vue";
import MkIcon from "./MkIcon.vue";

import type { MkErrorLike } from "../types";

const props = defineProps<{ title?: string; error: MkErrorLike | string | null | undefined }>();
const emit = defineEmits<{ retry: [] }>();
const text = useUiText();
const normalized = computed<MkErrorLike>(() =>
  typeof props.error === "string" ? { message: props.error } : (props.error ?? { message: "" }),
);
</script>

<template>
  <div class="mk-state mk-state--error" role="alert">
    <MkIcon name="alert" :size="28" />
    <p class="mk-state__title">{{ title ?? text("loadFailed") }}</p>
    <p v-if="normalized.message" class="mk-state__description">{{ normalized.message }}</p>
    <p v-if="normalized.code" class="mk-state__code">
      {{ text("errorCode", { code: normalized.code }) }}
    </p>
    <div class="mk-state__actions">
      <MkButton icon="refresh" @click="emit('retry')">{{ text("retry") }}</MkButton>
      <slot name="actions" />
    </div>
  </div>
</template>
