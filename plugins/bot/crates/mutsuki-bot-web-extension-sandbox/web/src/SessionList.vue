<script setup lang="ts">
import { MkIconButton, MkSearchInput, MkSegmented } from "@mutsuki/ui";
import { formatRelativeTime, useMessages, useSession } from "@mutsuki/web-sdk";
import { computed, ref } from "vue";

import { kindLabel } from "./content";
import type { SandboxMode } from "./contract";
import { messages } from "./messages";
import SandboxAvatar from "./SandboxAvatar.vue";
import { useSandboxStore } from "./store";

const store = useSandboxStore();
const session = useSession();
const { t } = useMessages(messages);
const query = ref("");

const modes = computed(() => [
  { value: "simulate" as SandboxMode, label: t("modes.simulate") },
  { value: "live" as SandboxMode, label: t("modes.live"), disabled: store.data.value?.live_available === false },
]);
const items = computed(() => {
  const needle = query.value.trim().toLowerCase();
  return (store.data.value?.conversations ?? []).filter(
    (item) =>
      !needle ||
      item.title.toLowerCase().includes(needle) ||
      item.users.some((user) => (user.display_name || "").toLowerCase().includes(needle)),
  );
});

function setMode(mode: SandboxMode) {
  if (mode !== store.mode.value) void store.write({ action: "set_mode", mode });
}
</script>

<template>
  <section class="sandbox-pane" :aria-label="t('sessions.search')">
    <div class="sandbox-pane-head">
      <MkSegmented :label="t('modes.label')" :options="modes" :model-value="store.mode.value" @update:model-value="setMode" />
    </div>
    <div class="sandbox-search-row">
      <MkSearchInput :label="t('sessions.search')" @search="query = $event" />
      <MkIconButton
        v-if="!store.live.value"
        icon="plus"
        :label="t('sessions.addUser')"
        :disabled="!session.can('bot.sandbox.write')"
        @click="store.write({ action: 'add_user' })"
      />
    </div>
    <div class="sandbox-session-list" role="list">
      <p v-if="!items.length" class="muted sandbox-empty">{{ t("sessions.empty") }}</p>
      <button
        v-for="item in items"
        :key="item.conversation_id"
        type="button"
        role="listitem"
        :class="['sandbox-session', { 'is-active': item.conversation_id === store.selectedId.value }]"
        :aria-current="item.conversation_id === store.selectedId.value ? 'true' : undefined"
        @click="store.selectedId.value = item.conversation_id"
      >
        <SandboxAvatar :name="item.title || kindLabel(item.kind)" :url="item.avatar_url || item.users[0]?.avatar_url" />
        <div>
          <span class="sandbox-session-title">{{ item.title || kindLabel(item.kind) }}</span>
          <p class="muted sandbox-session-preview">{{ kindLabel(item.kind) }} · {{ item.last_preview || t("sessions.noMessages") }}</p>
        </div>
        <span class="muted sandbox-session-time">{{ item.last_activity_unix_ms ? formatRelativeTime(item.last_activity_unix_ms) : "" }}</span>
      </button>
    </div>
  </section>
</template>
