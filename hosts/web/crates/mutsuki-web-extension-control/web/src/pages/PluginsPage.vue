<script setup lang="ts">
import { MkAsync, MkButton, MkCard } from "@mutsuki/ui";
import { pluginDisplayName, useMessages, type ExtensionContext } from "@mutsuki/web-sdk";

import { useControlAction, useControlQuery } from "../composables";
import { controlApi, type DeploymentCandidate, type PluginEntry } from "../contract";
import { messages } from "../messages";

const props = defineProps<{ registries: Pick<ExtensionContext, "pages" | "navigation"> }>();
const { t } = useMessages(messages);
const list = useControlQuery(controlApi.plugin_list, ["plugins"]);
const run = useControlAction();

const title = (id: string) => pluginDisplayName(props.registries, id);
const hasHub = (id: string) => props.registries.pages.list().some((page) => page.id === id);

function reload() {
  void run(controlApi.plugin_reload, undefined, {
    confirm: { title: t("plugins.reload"), message: t("plugins.reloadConfirm") },
    refresh: list.refresh,
  });
}

function prefer(plugin: PluginEntry, candidate: DeploymentCandidate) {
  void run(
    controlApi.plugin_deployment_set,
    { plugin_id: plugin.plugin_id, deployment: candidate.deployment },
    {
      confirm: {
        title: t("plugins.prefer"),
        message: t("plugins.preferConfirm", { plugin: plugin.plugin_id, deployment: candidate.deployment }),
      },
      refresh: list.refresh,
    },
  );
}

function clearPreference(plugin: PluginEntry) {
  void run(controlApi.plugin_deployment_clear, { plugin_id: plugin.plugin_id }, { refresh: list.refresh });
}
</script>

<template>
  <div class="page-body">
    <div class="toolbar row-item">
      <MkButton icon="refresh" :loading="list.loading.value" @click="list.refresh()">{{ t("refresh") }}</MkButton>
      <MkButton @click="reload">{{ t("plugins.reload") }}</MkButton>
      <span v-if="list.error.value && list.data.value" class="err-text">{{ t("refreshFailed") }}</span>
    </div>
    <MkAsync :source="list" :is-empty="(value) => !value.plugins.length && !value.diagnostics?.length" :empty-title="t('plugins.empty')">
      <template #default="{ data }">
        <MkCard v-if="data.diagnostics?.length" :title="t('plugins.diagnostics')">
          <div v-for="(item, index) in data.diagnostics" :key="index" class="tree-item">
            <strong>{{ item.plugin_id || item.manifest_path || t("plugins.manifest") }}</strong>
            <div class="muted">{{ item.detail || "—" }}</div>
          </div>
        </MkCard>
        <MkCard v-for="plugin in data.plugins" :key="plugin.plugin_id" :title="title(plugin.plugin_id)">
          <template #actions>
            <a v-if="hasHub(plugin.plugin_id)" class="mk-button mk-button--ghost mk-button--sm" :href="`#/plugins/${plugin.plugin_id}`">
              {{ t("plugins.open") }}
            </a>
          </template>
          <p class="muted">
            {{ t("plugins.active", { id: plugin.plugin_id, deployment: plugin.active_deployment || "—" }) }}
          </p>
          <div v-for="candidate in plugin.candidates ?? []" :key="candidate.deployment" class="tree-item row-item">
            <div>
              <strong>{{ candidate.deployment }}</strong>
              <div class="muted">
                {{
                  t("plugins.candidate", {
                    version: candidate.version || "—",
                    availability: candidate.available ? t("plugins.available") : t("plugins.unavailable"),
                  })
                }}
              </div>
            </div>
            <MkButton size="sm" variant="ghost" :disabled="!candidate.available" @click="prefer(plugin, candidate)">
              {{ t("plugins.prefer") }}
            </MkButton>
          </div>
          <MkButton v-if="plugin.preferred_deployment" size="sm" variant="ghost" @click="clearPreference(plugin)">
            {{ t("plugins.clear") }}
          </MkButton>
        </MkCard>
      </template>
    </MkAsync>
  </div>
</template>
