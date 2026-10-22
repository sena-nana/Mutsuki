<script setup lang="ts">
import { MkButton, MkDialog, MkField, MkIconButton, MkTextInput, useConfirm, useToast } from "@mutsuki/ui";
import { useMessages, useSession } from "@mutsuki/web-sdk";
import { computed, onBeforeUnmount, reactive, ref, watch } from "vue";

import { botProfile, memberRoster } from "./content";
import type { SandboxUser } from "./contract";
import { messages } from "./messages";
import SandboxAvatar from "./SandboxAvatar.vue";
import { useSandboxStore } from "./store";

/**
 * Who speaks and who is in the conversation. In simulate mode a member is picked as the
 * speaker, members can be edited, removed or imported from live data, and a private chat edits
 * its one user in place. Every member has a menu: mention, copy OpenID, import as simulated.
 */
const store = useSandboxStore();
const session = useSession();
const confirm = useConfirm();
const toast = useToast();
const { t } = useMessages(messages);
const canWrite = computed(() => session.can("bot.sandbox.write"));
const simulate = computed(() => !store.live.value);
const bot = computed(() => botProfile(store.data.value).user_id);
const roster = computed(() => memberRoster(store.conversation.value, store.data.value, simulate.value));
const privateUser = computed(() => (simulate.value && store.conversation.value?.kind === "private" ? store.conversation.value.users[0] : null));

const identity = reactive({ userId: "", openid: "", nickname: "" });
watch(
  privateUser,
  (user) => Object.assign(identity, { userId: user?.user_id ?? "", openid: user?.user_id ?? "", nickname: user?.display_name ?? "" }),
  { immediate: true },
);

const editor = reactive({ open: false, userId: "", openid: "", nickname: "" });
const importer = ref(false);
const menu = ref<{ user: SandboxUser; x: number; y: number } | null>(null);
const taken = computed(() => new Set(store.conversation.value?.users.map((user) => user.user_id)));
const liveUsers = computed(() => store.data.value?.live_users ?? []);

async function saveUser(userId: string, openid: string, nickname: string) {
  const newId = openid.trim();
  const wasPrivate = store.conversation.value?.conversation?.user_id === userId;
  const written = await store.write({ action: "update_user", user_id: userId, new_user_id: newId, display_name: nickname.trim() });
  if (!written) return false;
  if (store.identityId.value === userId) store.identityId.value = newId;
  if (wasPrivate) {
    const next = store.data.value?.conversations.find((item) => item.conversation?.user_id === newId);
    if (next) store.selectedId.value = next.conversation_id;
  }
  return true;
}

async function saveIdentity() {
  if (await saveUser(identity.userId, identity.openid, identity.nickname)) toast.success(t("members.saved"));
}

async function saveEditor() {
  if (await saveUser(editor.userId, editor.openid, editor.nickname)) editor.open = false;
}

function edit(user: SandboxUser) {
  Object.assign(editor, { open: true, userId: user.user_id, openid: user.user_id, nickname: user.display_name });
}

async function remove(user: SandboxUser) {
  const name = user.display_name || user.user_id;
  if (!(await confirm({ title: t("members.remove"), message: t("members.removeConfirm", { name }), confirmLabel: t("members.remove"), danger: true }))) return;
  if ((await store.write({ action: "remove_user", user_id: user.user_id })) && store.identityId.value === user.user_id) store.identityId.value = "";
}

async function importUsers(userIds: string[]) {
  if (!userIds.length) {
    toast.info(t("members.nothingToImport"));
    return;
  }
  if (await store.write({ action: "import_live_users", user_ids: userIds })) importer.value = false;
}

function selectIdentity(user: SandboxUser) {
  if (!simulate.value || user.user_id === bot.value) return;
  store.identityId.value = user.user_id;
  store.settleQuote();
}

function openMenu(user: SandboxUser, event: MouseEvent) {
  const host = (event.currentTarget as HTMLElement).closest(".sandbox-client")!.getBoundingClientRect();
  menu.value = { user, x: event.clientX - host.left, y: event.clientY - host.top };
}

async function menuAction(action: "mention" | "copy" | "import") {
  const user = menu.value!.user;
  menu.value = null;
  if (action === "mention") {
    store.segments.value.push({ type: "mention_user", user_id: user.user_id });
  } else if (action === "copy") {
    try {
      await navigator.clipboard.writeText(user.user_id);
      toast.success(t("members.copied"));
    } catch (error) {
      store.fail(error);
    }
  } else {
    const written = await store.write({ action: "import_live_users", user_ids: [user.user_id] });
    if (!written) return;
    const imported = (written.result as { imported?: unknown[] } | null)?.imported ?? [];
    if (!imported.length) {
      toast.info(t("members.alreadyImported"));
      return;
    }
    store.identityId.value = user.user_id;
    if (await store.write({ action: "set_mode", mode: "simulate" })) toast.success(t("members.imported"));
  }
}

const closeMenu = (event: PointerEvent) => {
  if (!(event.target as Element).closest(".sandbox-member-menu")) menu.value = null;
};
document.addEventListener("pointerdown", closeMenu, true);
onBeforeUnmount(() => document.removeEventListener("pointerdown", closeMenu, true));
</script>

<template>
  <section class="sandbox-pane" :aria-label="simulate ? t('members.identity') : t('members.title')">
    <template v-if="privateUser">
      <div class="sandbox-pane-head"><h2>{{ t("members.identity") }}</h2></div>
      <form class="sandbox-identity" @submit.prevent="saveIdentity">
        <MkField :label="t('members.openid')"><MkTextInput v-model="identity.openid" :disabled="!canWrite" /></MkField>
        <MkField :label="t('members.nickname')"><MkTextInput v-model="identity.nickname" :disabled="!canWrite" /></MkField>
        <MkButton type="submit" size="sm" class="sandbox-identity-save" :disabled="!canWrite">{{ t("members.save") }}</MkButton>
      </form>
    </template>
    <template v-else>
      <div class="sandbox-pane-head">
        <h2>{{ simulate ? t("members.identity") : t("members.title") }}</h2>
        <MkButton v-if="simulate" size="sm" variant="ghost" :disabled="!canWrite" @click="importer = true">{{ t("members.import") }}</MkButton>
      </div>
      <div class="sandbox-member-list" role="list">
        <p v-if="store.conversation.value && !roster.length" class="muted sandbox-empty">{{ t("members.empty") }}</p>
        <div
          v-for="user in roster"
          :key="user.user_id"
          role="listitem"
          :class="[
            'sandbox-member',
            { 'is-bot': user.user_id === bot, 'is-active': simulate && user.user_id !== bot && user.user_id === store.identityId.value },
          ]"
          @contextmenu.prevent="user.user_id !== bot && openMenu(user, $event)"
        >
          <SandboxAvatar :name="user.display_name || user.user_id" :url="user.avatar_url" small />
          <button
            v-if="simulate && user.user_id !== bot"
            type="button"
            class="sandbox-member-pick"
            :aria-pressed="user.user_id === store.identityId.value"
            @click="selectIdentity(user)"
          >
            <span class="sandbox-member-name">{{ user.display_name || user.user_id }}</span>
          </button>
          <div v-else>
            <span class="sandbox-member-name">{{ user.display_name || user.user_id }}</span>
            <p v-if="user.user_id === bot" class="muted sandbox-member-note">{{ t("members.bot") }}</p>
          </div>
          <div v-if="user.user_id !== bot" class="sandbox-member-actions">
            <MkButton v-if="simulate" size="sm" variant="ghost" :disabled="!canWrite" @click="edit(user)">{{ t("members.edit") }}</MkButton>
            <MkButton v-if="simulate" size="sm" variant="ghost" :disabled="!canWrite" @click="remove(user)">{{ t("members.remove") }}</MkButton>
            <MkIconButton icon="more" size="sm" :label="t('members.actions', { name: user.display_name || user.user_id })" @click="openMenu(user, $event)" />
          </div>
        </div>
      </div>
    </template>
    <div v-if="menu" class="sandbox-context-menu sandbox-member-menu" role="menu" :style="{ left: `${menu.x}px`, top: `${menu.y}px` }">
      <button type="button" role="menuitem" @click="menuAction('mention')">{{ t("members.mention", { name: menu.user.display_name || menu.user.user_id }) }}</button>
      <button type="button" role="menuitem" @click="menuAction('copy')">{{ t("members.copy") }}</button>
      <button v-if="store.live.value" type="button" role="menuitem" :disabled="!canWrite" @click="menuAction('import')">
        {{ t("members.importAsSimulated") }}
      </button>
    </div>
    <MkDialog v-model:open="editor.open" :title="t('members.editTitle')">
      <form class="mk-overlay__form" @submit.prevent="saveEditor">
        <MkField :label="t('members.openid')"><MkTextInput v-model="editor.openid" /></MkField>
        <MkField :label="t('members.nickname')"><MkTextInput v-model="editor.nickname" /></MkField>
      </form>
      <template #footer>
        <MkButton variant="primary" @click="saveEditor">{{ t("members.save") }}</MkButton>
      </template>
    </MkDialog>
    <MkDialog v-model:open="importer" :title="t('members.importTitle')">
      <p v-if="!liveUsers.length" class="muted">{{ t("members.importEmpty") }}</p>
      <button
        v-for="user in liveUsers"
        :key="user.user_id"
        type="button"
        class="sandbox-member"
        :disabled="taken.has(user.user_id)"
        @click="importUsers([user.user_id])"
      >
        <SandboxAvatar :name="user.display_name || user.user_id" :url="user.avatar_url" small />
        <div>
          <span class="sandbox-member-name">{{ user.display_name || user.user_id }}</span>
          <p class="muted sandbox-session-preview">{{ user.user_id }}</p>
        </div>
      </button>
      <template #footer>
        <MkButton v-if="liveUsers.length" variant="primary" @click="importUsers(liveUsers.map((user) => user.user_id).filter((id) => !taken.has(id)))">
          {{ t("members.importAll") }}
        </MkButton>
      </template>
    </MkDialog>
  </section>
</template>
