<script setup lang="ts">
import { MkCard, MkEmptyState } from "@mutsuki/ui";
import {
  pluginIdsOf,
  useMessages,
  useRpc,
  type ExtensionContext,
  type Registry,
  type SlotRegistration,
} from "@mutsuki/web-sdk";
import { computed, markRaw, type Component } from "vue";

import ConfigEditor from "./ConfigEditor.vue";
import { configApi, controlPlugins } from "./contract";
import { messages } from "./messages";
import type { FieldRenderers } from "./renderers";

/**
 * `#/plugins/<id>`: what one plugin contributes. Its version and deployment, its overview
 * cards and extra pages, then its configuration — the owner's `config.form` when it registered
 * one, otherwise the generated SchemaForm.
 */
const props = defineProps<{
  pluginId: string;
  hasSchema: boolean;
  slots: Pick<Registry<SlotRegistration>, "list">;
  registries: Pick<ExtensionContext, "pages" | "navigation">;
  renderers: FieldRenderers;
}>();
const { t } = useMessages(messages);
const owned = (item: { pluginId?: string; pluginIds?: string[] }) => pluginIdsOf(item).includes(props.pluginId);

const plugins = useRpc(controlPlugins.plugin_list);
const metrics = props.hasSchema ? useRpc(configApi.metrics) : null;
const plugin = computed(() => plugins.data.value?.plugins.find((item) => item.plugin_id === props.pluginId));
const meta = computed(() => {
  const entry = plugin.value;
  if (!entry) return "";
  const version =
    entry.candidates?.find((candidate) => candidate.deployment === entry.active_deployment)?.version ?? entry.candidates?.[0]?.version;
  return t("hub.meta", { plugin: entry.plugin_id, version: version || "—", deployment: entry.active_deployment || "—" });
});

const cards = computed(() =>
  props.slots
    .list()
    .filter((item) => item.slot === "overview.cards" && owned(item))
    .map((item) => ({ id: item.id, component: markRaw(item.component as Component) })),
);
const form = computed(() => {
  const item = props.slots.list().find((entry) => entry.slot === "config.form" && owned(entry));
  return item ? markRaw(item.component as Component) : null;
});
const pages = computed(() => {
  const nav = new Map(props.registries.navigation.list().map((item) => [item.pageId, item]));
  return props.registries.pages
    .list()
    .filter((page) => page.id !== props.pluginId && owned(page))
    .map((page) => {
      const entry = nav.get(page.id);
      return { id: page.id, label: entry?.label || page.title, href: entry ? `#/${entry.activityId}/${page.id}` : `#${page.path}` };
    });
});
const empty = computed(() => !props.hasSchema && !form.value && !cards.value.length && !pages.value.length);
</script>

<template>
  <div class="page-body plugin-hub">
    <p v-if="meta" class="muted mono">{{ meta }}</p>
    <MkEmptyState v-if="empty" :title="t('hub.empty')" />
    <div v-if="cards.length" class="overview-cards">
      <MkCard v-for="card in cards" :key="card.id">
        <component :is="card.component" />
      </MkCard>
    </div>
    <MkCard v-if="pages.length" :title="t('hub.pages')">
      <div class="toolbar">
        <a v-for="page in pages" :key="page.id" class="mk-button mk-button--secondary mk-button--sm" :href="page.href">{{ page.label }}</a>
      </div>
    </MkCard>
    <component :is="form" v-if="form" :provider-id="pluginId" />
    <ConfigEditor v-else-if="hasSchema" :provider-id="pluginId" :renderers="renderers" />
    <p v-if="metrics?.data.value" class="muted">
      {{ t("hub.metrics") }}：{{
        t("hub.metricsLine", {
          providers: metrics.data.value.config_provider_count,
          conflicts: metrics.data.value.config_revision_conflict,
          failed: metrics.data.value.config_apply_failed,
          latency: metrics.data.value.config_apply_latency_avg_ms,
        })
      }}
    </p>
  </div>
</template>

<style>
.plugin-hub {
  display: grid;
  gap: var(--mk-space-4);
}
</style>
