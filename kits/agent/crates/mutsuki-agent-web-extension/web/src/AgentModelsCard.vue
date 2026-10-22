<script setup lang="ts">
import { MkStatusBadge } from "@mutsuki/ui";
import { useMessages, useRpc } from "@mutsuki/web-sdk";

import { agentApi } from "./contract";
import { messages } from "./messages";

/** Overview card: whether the model connections are up. */
const { t } = useMessages(messages);
const snapshot = useRpc(agentApi["connections.snapshot"]);
</script>

<template>
  <section class="agent-card" :aria-label="t('card.title')">
    <h2 class="mk-card__title">{{ t("card.title") }}</h2>
    <p v-if="snapshot.error.value || !snapshot.data.value?.connections.length" class="muted">
      {{ snapshot.loading.value ? "" : t("card.empty") }}
    </p>
    <div v-for="item in snapshot.data.value?.connections ?? []" :key="item.connection_id" class="tree-item row-item">
      <strong>{{ item.connection_id === "local" ? t("connections.local") : item.connection_id }}</strong>
      <MkStatusBadge
        :tone="item.state === 'healthy' ? 'ok' : 'warn'"
        :label="t(`connections.states.${item.state}` as never)"
      />
    </div>
  </section>
</template>
