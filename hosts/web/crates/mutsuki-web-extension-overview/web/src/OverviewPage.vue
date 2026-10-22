<script setup lang="ts">
import { MkAsync, MkIconButton, MkStatusBadge, type MkTone } from "@mutsuki/ui";
import {
  formatDuration,
  useMessages,
  usePolling,
  useRpc,
  type Registry,
  type SlotRegistration,
} from "@mutsuki/web-sdk";
import { computed, onBeforeUnmount, ref, watch } from "vue";

import { controlChanged, overviewApi, type OverviewSummary } from "./contract";
import { messages } from "./messages";
import SlotCard from "./SlotCard.vue";

const props = defineProps<{ slots: Pick<Registry<SlotRegistration>, "list"> }>();
const { t } = useMessages(messages);
const summary = useRpc(overviewApi.summary, undefined, { refreshOn: [controlChanged] });
// Events carry every change; the minute poll only reconciles anything a dropped event missed.
usePolling(() => summary.refresh(), 60_000, { immediate: false });

const cards = computed(() => props.slots.list().filter((item) => item.slot === "overview.cards"));

// Uptime keeps counting between snapshots from the moment the last one arrived.
const receivedAt = ref(Date.now());
const now = ref(Date.now());
watch(summary.data, () => (receivedAt.value = Date.now()));
const ticker = setInterval(() => (now.value = Date.now()), 1_000);
onBeforeUnmount(() => clearInterval(ticker));

function uptime(data: OverviewSummary): string {
  return formatDuration(Number(data.uptime_ms ?? 0) + Math.max(0, now.value - receivedAt.value));
}

function health(data: OverviewSummary): { label: string; tone: MkTone } {
  switch (String(data.health?.service ?? data.health?.core ?? "").toLowerCase()) {
    case "ok":
    case "healthy":
      return { label: t("health.ok"), tone: "ok" };
    case "degraded":
      return { label: t("health.degraded"), tone: "warn" };
    case "unhealthy":
    case "failed":
      return { label: t("health.failed"), tone: "err" };
    case "stopped":
      return { label: t("health.stopped"), tone: "err" };
    default:
      return { label: t("health.unknown"), tone: "neutral" };
  }
}

const count = (value: number | null | undefined) => (value == null ? "—" : String(value));
</script>

<template>
  <div class="page-body overview-dashboard">
    <Teleport to="#mk-page-actions" defer>
      <MkIconButton
        icon="refresh"
        :label="summary.error.value && summary.data.value ? t('refreshFailed') : t('refresh')"
        @click="summary.refresh()"
      />
    </Teleport>
    <div v-if="cards.length" class="overview-cards">
      <SlotCard v-for="item in cards" :key="item.id" :item="item" />
    </div>
    <MkAsync :source="summary">
      <template #default="{ data }">
        <div class="metric-grid">
          <div class="metric-card">
            <div class="metric-label">{{ t("uptime") }}</div>
            <div class="metric-value">{{ uptime(data) }}</div>
          </div>
          <div class="metric-card">
            <div class="metric-label">{{ t("status") }}</div>
            <div class="metric-value">
              <MkStatusBadge :tone="health(data).tone" :label="health(data).label" />
            </div>
          </div>
          <div class="metric-card">
            <div class="metric-label">{{ t("plugins") }}</div>
            <div class="metric-value">{{ count(data.counts?.plugins) }}</div>
          </div>
          <div class="metric-card">
            <div class="metric-label">{{ t("runners") }}</div>
            <div class="metric-value">{{ count(data.counts?.runners) }}</div>
          </div>
          <div class="metric-card">
            <div class="metric-label">{{ t("eventSources") }}</div>
            <div class="metric-value">{{ count(data.counts?.event_sources) }}</div>
          </div>
        </div>
      </template>
    </MkAsync>
  </div>
</template>
