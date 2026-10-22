<script setup lang="ts">
import { MkButton, MkCard } from "@mutsuki/ui";
import { useMessages } from "@mutsuki/web-sdk";

import { useControlAction } from "../composables";
import { controlApi } from "../contract";
import { messages } from "../messages";

const { t } = useMessages(messages);
const run = useControlAction();

function drain() {
  void run(controlApi.core_begin_drain, undefined, {
    confirm: {
      title: t("lifecycle.drainTitle"),
      message: t("lifecycle.drainHint"),
      confirmLabel: t("lifecycle.drain"),
      danger: true,
      requireText: t("lifecycle.drainText"),
    },
    success: t("lifecycle.drained"),
  });
}

function shutdown() {
  void run(controlApi.service_shutdown, undefined, {
    confirm: {
      title: t("lifecycle.shutdownTitle"),
      message: t("lifecycle.shutdownHint"),
      confirmLabel: t("lifecycle.shutdown"),
      danger: true,
      requireText: t("lifecycle.shutdownText"),
    },
    success: t("lifecycle.shutdownSent"),
  });
}
</script>

<template>
  <div class="page-body">
    <MkCard :title="t('lifecycle.drainTitle')">
      <p class="muted">{{ t("lifecycle.drainHint") }}</p>
      <MkButton @click="drain">{{ t("lifecycle.drain") }}</MkButton>
    </MkCard>
    <MkCard :title="t('lifecycle.shutdownTitle')">
      <p class="muted">{{ t("lifecycle.shutdownHint") }}</p>
      <MkButton variant="danger" @click="shutdown">{{ t("lifecycle.shutdown") }}</MkButton>
    </MkCard>
  </div>
</template>
