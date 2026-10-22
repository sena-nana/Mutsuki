<script setup lang="ts">
import { MkErrorState, MkSkeleton, MkStatusBadge, type MkTone } from "@mutsuki/ui";
import { formatDuration, useMessages, usePolling, useRpc } from "@mutsuki/web-sdk";
import { computed, ref } from "vue";

import { qqApi, qqChanged, type QqAccount } from "./contract";
import { messages } from "./messages";

/** Overview card: who is online. Account management lives on the platforms page. */
const { t } = useMessages(messages);
const snapshot = useRpc(qqApi.snapshot, { query: "" }, { refreshOn: [qqChanged] });
usePolling(() => snapshot.refresh(), 60_000, { immediate: false });
const failedAvatars = ref(new Set<string>());

/** The owner is not running (no QQ account configured): a normal state, not an error. */
const notConfigured = computed(() => snapshot.error.value?.code === "qq.owner_unavailable");
const name = (account: QqAccount) => account.self_user?.display_name || account.account_id;
const tone = (health: string): MkTone => (health === "ok" ? "ok" : health === "degraded" ? "warn" : "err");
const KNOWN = {
  health: ["ok", "degraded", "unhealthy", "stopped"],
  connection: ["reconnecting", "disconnected", "connected", "identified", "resumable"],
};
const label = (group: keyof typeof KNOWN, value: string) =>
  KNOWN[group].includes(value) ? t(`${group}.${value}` as never) : value;

/** QQ CDN avatars are served over https; anything else falls back to the initial. */
function avatar(account: QqAccount): string | null {
  const raw = String(account.self_user?.avatar_url ?? "").trim();
  if (!raw || failedAvatars.value.has(account.account_id)) return null;
  const href = raw.startsWith("//") ? `https:${raw}` : raw.replace(/^http:\/\//i, "https://");
  try {
    const host = new URL(href).hostname;
    return /(^|\.)(qlogo\.cn|qpic\.cn|gtimg\.cn|qq\.com\.cn|qq\.com)$/i.test(host) ? href : null;
  } catch {
    return null;
  }
}
</script>

<template>
  <section class="qq-card" :aria-label="t('card.title')">
    <h2 class="mk-card__title">{{ t("card.title") }}</h2>
    <p v-if="notConfigured" class="muted">{{ t("card.notConfigured") }}</p>
    <MkErrorState v-else-if="snapshot.error.value && !snapshot.data.value" :error="snapshot.error.value" @retry="snapshot.refresh()" />
    <MkSkeleton v-else-if="!snapshot.data.value" :lines="2" />
    <ul v-else class="qq-card__accounts">
      <li v-for="account in snapshot.data.value.accounts" :key="account.account_id" class="tree-item row-item">
        <img
          v-if="avatar(account)"
          class="qq-card__avatar"
          :src="avatar(account)!"
          :alt="t('card.avatar', { name: name(account) })"
          referrerpolicy="no-referrer"
          width="40"
          height="40"
          @error="failedAvatars.add(account.account_id)"
        />
        <span v-else class="qq-card__avatar" aria-hidden="true">{{ name(account).slice(0, 1).toUpperCase() }}</span>
        <div>
          <strong>{{ name(account) }}</strong>
          <MkStatusBadge
            :tone="tone(account.health)"
            :label="`${label('health', account.health)} · ${label('connection', account.connection_state)}`"
          />
          <p class="muted">
            {{ t("card.online", { duration: account.connected_since_unix_ms ? formatDuration(Date.now() - account.connected_since_unix_ms) : "—" }) }}
          </p>
          <p v-if="account.last_error" class="err-text">{{ account.last_error }}</p>
        </div>
      </li>
    </ul>
  </section>
</template>

<style>
.qq-card__accounts {
  display: grid;
  gap: var(--mk-space-2);
  margin: var(--mk-space-2) 0 0;
  padding: 0;
  list-style: none;
}
.qq-card__avatar {
  display: grid;
  flex: none;
  place-items: center;
  width: 40px;
  height: 40px;
  border-radius: 50%;
  background: var(--mk-color-accent-soft);
  color: var(--mk-color-accent-on-soft);
  font-weight: 600;
  object-fit: cover;
}
</style>
