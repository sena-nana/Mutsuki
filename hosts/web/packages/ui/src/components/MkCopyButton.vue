<script setup lang="ts">
import { onBeforeUnmount, ref } from "vue";
import { useUiText } from "../messages";
import MkIcon from "./MkIcon.vue";

const props = defineProps<{ text: string; label?: string }>();
const status = ref<"idle" | "copied" | "failed">("idle");
const t = useUiText();
let reset: ReturnType<typeof setTimeout> | undefined;

async function copy() {
  try {
    await navigator.clipboard.writeText(props.text);
    status.value = "copied";
  } catch {
    status.value = "failed";
  }
  clearTimeout(reset);
  reset = setTimeout(() => (status.value = "idle"), 2_000);
}
onBeforeUnmount(() => clearTimeout(reset));
</script>

<template>
  <button
    type="button"
    class="mk-copy mk-icon-button mk-icon-button--sm"
    :aria-label="label ?? t('copy')"
    :title="label ?? t('copy')"
    @click="copy"
  >
    <MkIcon :name="status === 'copied' ? 'check' : 'copy'" :size="14" />
    <span class="mk-visually-hidden" aria-live="polite">{{
      status === "copied" ? t("copied") : status === "failed" ? t("copyFailed") : ""
    }}</span>
  </button>
</template>
