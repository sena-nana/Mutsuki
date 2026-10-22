<script setup lang="ts">
import { MkButton, MkIconButton, useConfirm } from "@mutsuki/ui";
import { formatDateTime, useMessages, useSession } from "@mutsuki/web-sdk";
import { computed, nextTick, ref, watch } from "vue";

import Composer from "./Composer.vue";
import { botProfile, canQuote, groupMessages, kindLabel, rowLayout } from "./content";
import type { Message } from "./contract";
import MessageBody from "./MessageBody.vue";
import { messages as text } from "./messages";
import SandboxAvatar from "./SandboxAvatar.vue";
import { useSandboxStore } from "./store";

const store = useSandboxStore();
const session = useSession();
const confirm = useConfirm();
const { t } = useMessages(text);
const list = ref<HTMLElement | null>(null);

const rows = computed(() =>
  groupMessages(store.messages.value).map((group) => {
    const first = group[0];
    const sender = first.role === "bot" ? botProfile(store.data.value) : store.conversation.value?.users.find((user) => user.user_id === first.sender_id);
    return {
      key: first.message_id,
      group,
      layout: rowLayout(first, store.identityId.value, store.live.value),
      name: sender?.display_name || first.sender_name,
      avatar: sender?.avatar_url,
      system: first.role === "system",
    };
  }),
);

// Follow the conversation's end when messages arrive.
watch(
  () => store.messages.value,
  () => void nextTick(() => list.value && (list.value.scrollTop = list.value.scrollHeight)),
);

const quotable = (message: Message) => canQuote(message, store.identityId.value, store.live.value);

async function clear() {
  const conversation = store.conversation.value!;
  const ok = await confirm({
    title: t("chat.clear"),
    message: t("chat.clearConfirm", { title: conversation.title || kindLabel(conversation.kind) }),
    confirmLabel: t("chat.clear"),
    danger: true,
  });
  if (ok) await store.write({ action: "clear_conversation", conversation_id: conversation.conversation_id });
}
</script>

<template>
  <section class="sandbox-pane sandbox-pane--chat" :aria-label="store.conversation.value?.title || t('chat.title')">
    <div class="sandbox-pane-head">
      <h2>{{ store.conversation.value?.title || t("chat.title") }}</h2>
      <span v-if="store.conversation.value" class="muted">{{ kindLabel(store.conversation.value.kind) }}</span>
      <MkButton
        v-if="!store.live.value && store.conversation.value"
        size="sm"
        variant="ghost"
        :disabled="!session.can('bot.sandbox.write')"
        @click="clear"
      >
        {{ t("chat.clear") }}
      </MkButton>
    </div>
    <div ref="list" class="sandbox-messages" role="log" :aria-label="t('chat.title')">
      <p v-if="!store.conversation.value" class="muted sandbox-empty">{{ t("chat.pick") }}</p>
      <p v-else-if="!rows.length" class="muted sandbox-empty">{{ t("chat.empty") }}</p>
      <div v-for="row in rows" :key="row.key" :class="['sandbox-row', `sandbox-row--${row.layout}`]">
        <SandboxAvatar v-if="!row.system" :name="row.name" :url="row.avatar" small />
        <div class="sandbox-row-body">
          <div v-for="(message, index) in row.group" :key="message.message_id" class="sandbox-bubble">
            <p v-if="!row.system && index === 0" class="muted">{{ row.name }} · {{ formatDateTime(message.time_ms) }}</p>
            <MessageBody :message="message" :messages="store.messages.value" />
            <MkIconButton
              v-if="quotable(message)"
              class="sandbox-reply"
              icon="reply"
              size="sm"
              :label="t('chat.reply')"
              @click="store.quote.value = message"
            />
          </div>
        </div>
      </div>
    </div>
    <Composer v-if="store.conversation.value" :key="store.conversation.value.conversation_id" :conversation="store.conversation.value" />
  </section>
</template>
