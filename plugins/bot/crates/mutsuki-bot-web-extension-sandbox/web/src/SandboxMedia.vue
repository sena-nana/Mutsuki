<script setup lang="ts">
import { callRpc, useMutsukiRuntime, type RuntimeClient } from "@mutsuki/web-sdk";
import { computed, ref, watch } from "vue";

import { sandboxApi } from "./contract";
import { imageUrl } from "./content";

/** Sandbox-stored media and stickers load as blobs once per page and stay cached. */
const blobs = new Map<string, Promise<string>>();

function blobUrl(client: RuntimeClient, kind: "media" | "sticker", id: string): Promise<string> {
  const key = `${kind}:${id}`;
  let url = blobs.get(key);
  if (!url) {
    url = (kind === "media" ? callRpc(client, sandboxApi["media.get"], { media_id: id }) : callRpc(client, sandboxApi["sticker.get"], { sticker_id: id })).then(
      (blob) => {
        const bytes = Uint8Array.from(atob(blob.bytes || ""), (char) => char.charCodeAt(0));
        return URL.createObjectURL(new Blob([bytes], { type: blob.mime || "application/octet-stream" }));
      },
    );
    url.catch(() => blobs.delete(key));
    blobs.set(key, url);
  }
  return url;
}

const props = defineProps<{ mime?: string; url?: string; mediaId?: string; stickerId?: string; name: string; avatar?: boolean }>();
const { client } = useMutsukiRuntime();
const src = ref("");
const failed = ref(false);
const kind = computed(() => (props.mime?.startsWith("audio/") ? "audio" : props.mime?.startsWith("video/") ? "video" : "image"));

watch(
  () => [props.url, props.mediaId, props.stickerId],
  async () => {
    failed.value = false;
    src.value = imageUrl(props.url);
    if (src.value) return;
    const id = props.stickerId ?? props.mediaId;
    if (!id) {
      failed.value = !props.avatar;
      return;
    }
    try {
      src.value = await blobUrl(client, props.stickerId ? "sticker" : "media", id);
    } catch {
      failed.value = true;
    }
  },
  { immediate: true },
);
</script>

<template>
  <span v-if="failed" class="sandbox-file">[媒体不可用]</span>
  <audio v-else-if="kind === 'audio' && src" class="sandbox-media" controls :src="src" :aria-label="name" />
  <video v-else-if="kind === 'video' && src" class="sandbox-media" controls :src="src" :aria-label="name" />
  <img v-else-if="src" :class="avatar ? 'sandbox-avatar' : 'sandbox-media'" :src="src" :alt="name" referrerpolicy="no-referrer" @error="failed = true" />
</template>
