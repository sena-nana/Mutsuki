<script setup lang="ts">
import { MkButton, MkDialog, MkField, MkIconButton, MkTextarea, MkTextInput, useConfirm, useToast } from "@mutsuki/ui";
import { callRpc, useMessages, useSession } from "@mutsuki/web-sdk";
import { computed, ref } from "vue";

import {
  composePayload,
  draftLabel,
  fileToBase64,
  isKeyboard,
  keyboardSegment,
  keyboardSource,
  memberRoster,
  mentionQuery,
  resolveIdentity,
} from "./content";
import { sandboxApi, type Conversation, type Segment, type Sticker } from "./contract";
import { messages } from "./messages";
import SandboxMedia from "./SandboxMedia.vue";
import { useSandboxStore } from "./store";

/**
 * Message composer: text or Markdown, mentions (`@` opens a picker), uploaded media and
 * stickers, and QQ buttons on Markdown. Simulate mode speaks as the chosen member; live mode
 * sends as the bot to the real conversation after a confirmation.
 */
const props = defineProps<{ conversation: Conversation }>();
const store = useSandboxStore();
const session = useSession();
const confirm = useConfirm();
const toast = useToast();
const { t } = useMessages(messages);

const draft = ref("");
const segments = store.segments;
const markdown = ref(false);
const stickerOpen = ref(false);
const stickers = ref<Sticker[]>([]);
const keyboard = ref({ open: false, source: "" });
const sending = ref(false);
const imageInput = ref<HTMLInputElement | null>(null);
const fileInput = ref<HTMLInputElement | null>(null);
const stickerInput = ref<HTMLInputElement | null>(null);

const canWrite = computed(() => session.can(store.live.value ? "bot.delivery.write" : "bot.sandbox.write"));
const mention = computed(() => mentionQuery(draft.value));
const mentionHits = computed(() => {
  const query = mention.value?.query;
  if (query === undefined) return [];
  return [{ user_id: "__all__", display_name: t("compose.mentionAll") }, ...memberRoster(props.conversation, store.data.value, !store.live.value)].filter(
    (user) => (user.display_name || "").toLowerCase().includes(query) || user.user_id.toLowerCase().includes(query),
  );
});
const placeholder = computed(() => {
  if (markdown.value) return t("compose.placeholderMarkdown");
  if (store.live.value) return t(props.conversation.active_message === false ? "compose.placeholderPassive" : "compose.placeholderLive");
  const identity = props.conversation.users.find((user) => user.user_id === store.identityId.value);
  const name = identity?.display_name || identity?.user_id;
  return props.conversation.kind !== "private" && name ? t("compose.placeholderAs", { name }) : t("compose.placeholder");
});
const visibleStickers = computed(() => stickers.value.filter((item) => item.kind !== "qq_face" || !store.live.value));

function pickMention(user: { user_id: string }) {
  draft.value = draft.value.slice(0, mention.value!.index);
  segments.value.push(user.user_id === "__all__" ? { type: "mention_all" } : { type: "mention_user", user_id: user.user_id });
}

async function upload(file: File | null | undefined) {
  if (!file) return;
  try {
    const uploaded = await callRpc(store.client, sandboxApi["media.upload"], {
      name: file.name || "paste.png",
      mime: file.type || "image/png",
      bytes: await fileToBase64(file),
    });
    segments.value.push({
      type: "platform_specific",
      platform: "sandbox",
      kind: "media",
      payload: { media_id: uploaded.media_id, mime: uploaded.mime, name: uploaded.name },
    });
  } catch (error) {
    store.fail(error);
  }
}

function onPick(event: Event) {
  const input = event.target as HTMLInputElement;
  void upload(input.files?.[0]);
  input.value = "";
}

function onPaste(event: ClipboardEvent) {
  const image =
    [...(event.clipboardData?.files ?? [])].find((file) => file.type.startsWith("image/")) ??
    [...(event.clipboardData?.items ?? [])].find((item) => item.type.startsWith("image/"))?.getAsFile();
  if (!image) return;
  event.preventDefault();
  void upload(image);
}

async function loadStickers() {
  try {
    stickers.value = (await callRpc(store.client, sandboxApi["sticker.list"])) ?? [];
  } catch (error) {
    store.fail(error);
  }
}

async function toggleStickers() {
  stickerOpen.value = !stickerOpen.value;
  if (stickerOpen.value) await loadStickers();
}

async function uploadSticker(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = "";
  if (!file) return;
  try {
    await callRpc(store.client, sandboxApi["sticker.upload"], {
      name: file.name || "sticker.png",
      mime: file.type || "image/png",
      bytes: await fileToBase64(file),
    });
    await loadStickers();
  } catch (error) {
    store.fail(error);
  }
}

function pickSticker(item: Sticker) {
  segments.value.push(
    item.kind === "qq_face"
      ? { type: "platform_specific", platform: "qqbot", kind: "face", payload: { face_type: item.face_type ?? "", face_id: item.face_id ?? "" } }
      : { type: "platform_specific", platform: "sandbox", kind: "sticker", payload: { sticker_id: item.id, mime: item.mime, name: item.name } },
  );
  stickerOpen.value = false;
}

function toggleMarkdown() {
  markdown.value = !markdown.value;
  if (!markdown.value) segments.value = segments.value.filter((segment) => !isKeyboard(segment));
}

function openKeyboard() {
  markdown.value = true;
  keyboard.value = { open: true, source: keyboardSource(segments.value.find(isKeyboard)) };
}

function saveKeyboard() {
  const segment = keyboardSegment(keyboard.value.source);
  segments.value = [...segments.value.filter((item) => !isKeyboard(item)), ...(segment ? [segment] : [])];
  keyboard.value.open = false;
}

async function send() {
  const payload = composePayload(draft.value, segments.value, markdown.value);
  if (!payload.ok) {
    toast.info(t(`compose.${payload.reason}`));
    return;
  }
  const live = store.live.value;
  if (!live) {
    store.identityId.value = resolveIdentity(props.conversation, store.data.value, store.identityId.value);
    if (!store.identityId.value) {
      toast.info(t("compose.noIdentity"));
      return;
    }
  }
  store.settleQuote();
  const replyTo = store.quote.value?.message_id ?? null;
  if (live && !replyTo && props.conversation.active_message === false) {
    toast.info(t("compose.noActive"));
    return;
  }
  if (live && !(await confirm({ title: t("compose.sendLive"), message: t("compose.liveConfirm"), confirmLabel: t("compose.sendLive") }))) return;
  sending.value = true;
  try {
    const written = await store.write(
      {
        action: live ? "send_as_bot" : "ingest_as_user",
        conversation_id: props.conversation.conversation_id,
        ...(live ? {} : { user_id: store.identityId.value }),
        text: payload.text,
        segments: payload.segments,
        reply_to: replyTo,
      },
      live,
    );
    if (!written) return;
    draft.value = "";
    segments.value = [];
    markdown.value = false;
    stickerOpen.value = false;
    store.quote.value = null;
  } finally {
    sending.value = false;
  }
}

function onKeydown(event: KeyboardEvent) {
  if (event.key !== "Enter" || event.isComposing) return;
  if (markdown.value && !event.metaKey && !event.ctrlKey) return;
  event.preventDefault();
  void send();
}
</script>

<template>
  <div class="sandbox-compose" @paste="onPaste">
    <div v-if="store.quote.value" class="sandbox-quote-bar muted">
      <span>{{ t("chat.replying", { name: store.quote.value.sender_name }) }}</span>
      <MkButton size="sm" variant="ghost" @click="store.quote.value = null">{{ t("chat.cancel") }}</MkButton>
    </div>
    <div v-if="segments.length" class="sandbox-draft-chips">
      <span v-for="(segment, index) in segments" :key="index" class="sandbox-draft-chip">
        {{ draftLabel(segment, conversation.users) }}
        <MkIconButton
          icon="close"
          size="sm"
          :label="t('compose.remove', { item: draftLabel(segment, conversation.users) })"
          @click="segments.splice(index, 1)"
        />
      </span>
    </div>
    <div class="sandbox-compose-row">
      <MkIconButton icon="image" :label="t('compose.image')" :disabled="!canWrite" @click="imageInput?.click()" />
      <MkIconButton icon="attach" :label="t('compose.file')" :disabled="!canWrite" @click="fileInput?.click()" />
      <MkIconButton icon="smile" :label="t('compose.sticker')" :pressed="stickerOpen" @click="toggleStickers" />
      <MkIconButton icon="document" :label="t('compose.markdown')" :pressed="markdown" @click="toggleMarkdown" />
      <MkIconButton icon="grid" :label="t('compose.keyboard')" :pressed="segments.some(isKeyboard)" @click="openKeyboard" />
      <div class="sandbox-compose-field">
        <div v-if="mentionHits.length" class="sandbox-context-menu sandbox-mention-picker" role="listbox" :aria-label="t('compose.mentions')">
          <button v-for="user in mentionHits" :key="user.user_id" type="button" role="option" :aria-selected="false" @click="pickMention(user)">
            @{{ user.display_name || user.user_id }}
          </button>
        </div>
        <div v-if="stickerOpen" class="sandbox-context-menu sandbox-sticker-picker" :aria-label="t('compose.stickers')" role="group">
          <MkButton size="sm" class="sandbox-sticker-add" :disabled="!session.can('bot.sandbox.write')" @click="stickerInput?.click()">
            {{ t("compose.addSticker") }}
          </MkButton>
          <p v-if="!visibleStickers.length" class="muted sandbox-sticker-empty">{{ t("compose.noStickers") }}</p>
          <button
            v-for="item in visibleStickers"
            :key="item.id"
            type="button"
            class="sandbox-sticker-item"
            :aria-label="item.kind === 'qq_face' ? t('compose.face', { type: item.face_type ?? '?', id: item.face_id ?? '?' }) : item.name || t('compose.sticker')"
            @click="pickSticker(item)"
          >
            <span v-if="item.kind === 'qq_face'" class="sandbox-sticker-face">{{ t("compose.face", { type: item.face_type ?? "?", id: item.face_id ?? "?" }) }}</span>
            <SandboxMedia v-else :sticker-id="item.id" :name="item.name || t('compose.sticker')" />
          </button>
        </div>
        <MkTextarea
          v-if="markdown"
          v-model="draft"
          :rows="3"
          :label="t('compose.message')"
          :placeholder="placeholder"
          :disabled="!canWrite"
          @keydown="onKeydown"
        />
        <MkTextInput v-else v-model="draft" :label="t('compose.message')" :placeholder="placeholder" :disabled="!canWrite" @keydown="onKeydown" />
      </div>
      <MkButton variant="primary" :loading="sending" :disabled="!canWrite" @click="send">
        {{ store.live.value ? t("compose.sendLive") : t("compose.send") }}
      </MkButton>
    </div>
    <input ref="imageInput" type="file" accept="image/*" hidden @change="onPick" />
    <input ref="fileInput" type="file" hidden @change="onPick" />
    <input ref="stickerInput" type="file" accept="image/*" hidden @change="uploadSticker" />
    <MkDialog v-model:open="keyboard.open" :title="t('compose.keyboardTitle')">
      <MkField :label="t('compose.keyboardHint')">
        <MkTextarea v-model="keyboard.source" :rows="5" />
      </MkField>
      <template #footer>
        <MkButton variant="primary" @click="saveKeyboard">{{ t("compose.save") }}</MkButton>
      </template>
    </MkDialog>
  </div>
</template>
