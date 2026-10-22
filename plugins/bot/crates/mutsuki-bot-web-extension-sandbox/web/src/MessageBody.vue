<script setup lang="ts">
import { computed } from "vue";

import type { Message } from "./contract";
import { imageUrl, messageParts, quotedText } from "./content";
import SandboxMedia from "./SandboxMedia.vue";

/** A message's quote and content, rendered as text nodes and media elements, never markup. */
const props = defineProps<{ message: Message; messages: Message[] }>();
const parts = computed(() => messageParts(props.message));
</script>

<template>
  <p v-if="message.reply_to" class="sandbox-quote muted">{{ quotedText(messages, message.reply_to) }}</p>
  <div class="sandbox-rich">
    <template v-for="(part, index) in parts" :key="index">
      <template v-if="part.kind === 'text'">{{ part.text }}</template>
      <span v-else-if="part.kind === 'mention'" class="sandbox-mention">{{ part.text }}</span>
      <SandboxMedia v-else-if="part.kind === 'media'" :mime="part.mime" :url="part.url" :media-id="part.mediaId" :name="part.name" />
      <SandboxMedia v-else-if="part.kind === 'sticker'" :sticker-id="part.stickerId" :name="part.name" />
      <span v-else-if="part.kind === 'file'" class="sandbox-file">[{{ part.name }}]</span>
      <div v-else-if="part.kind === 'card'" class="sandbox-card">
        <img v-if="imageUrl(part.image)" :src="imageUrl(part.image)" :alt="part.title" referrerpolicy="no-referrer" />
        <div class="sandbox-card-body">
          <span class="sandbox-card-title">{{ part.title }}</span>
          <span v-if="part.desc" class="muted sandbox-card-desc">{{ part.desc }}</span>
          <span v-if="part.url" class="muted sandbox-card-desc">{{ part.url }}</span>
        </div>
      </div>
      <pre v-else-if="part.kind === 'markdown'" class="sandbox-markdown">{{ part.content }}</pre>
      <div v-else-if="part.kind === 'keyboard'" class="sandbox-keyboard">
        <span v-for="(label, i) in part.labels" :key="i">{{ label }}</span>
      </div>
      <template v-else>{{ part.label }}</template>
    </template>
  </div>
</template>
