<script setup lang="ts">
import { computed, ref } from "vue";

import { imageUrl } from "./content";

/** https avatar when there is one that loads, else the name's initial. */
const props = defineProps<{ name: string; url?: string | null; small?: boolean }>();
const broken = ref(false);
const src = computed(() => (broken.value ? "" : imageUrl(props.url)));
const initial = computed(() => (props.name.trim().slice(0, 1) || "?").toUpperCase());
</script>

<template>
  <img
    v-if="src"
    :class="['sandbox-avatar', { 'sandbox-avatar--sm': small }]"
    :src="src"
    :alt="`${name} 的头像`"
    referrerpolicy="no-referrer"
    @error="broken = true"
  />
  <span v-else :class="['sandbox-avatar', { 'sandbox-avatar--sm': small }]" aria-hidden="true">{{ initial }}</span>
</template>
