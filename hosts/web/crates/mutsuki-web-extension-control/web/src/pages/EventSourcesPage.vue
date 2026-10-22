<script setup lang="ts">
import { MkAsync, MkButton } from "@mutsuki/ui";
import { useMessages } from "@mutsuki/web-sdk";

import { useControlAction, useControlQuery, useStateLabel } from "../composables";
import { controlApi, type EventSourceEntry } from "../contract";
import { messages } from "../messages";

const { t } = useMessages(messages);
const sources = useControlQuery(controlApi.event_source_list, ["event_sources"]);
const run = useControlAction();
const state = useStateLabel();

function restart(source: EventSourceEntry) {
  void run(controlApi.event_source_restart, { id: source.source_id }, {
    confirm: { title: t("events.restart"), message: t("events.restartConfirm", { id: source.source_id }) },
    refresh: sources.refresh,
  });
}
</script>

<template>
  <div class="page-body">
    <div class="toolbar row-item">
      <MkButton icon="refresh" :loading="sources.loading.value" @click="sources.refresh()">{{ t("refresh") }}</MkButton>
    </div>
    <MkAsync :source="sources" :empty-title="t('events.empty')">
      <template #default="{ data }">
        <div v-for="source in data" :key="source.source_id" class="tree-item row-item">
          <div>
            <strong>{{ source.source_id }}</strong>
            <div class="muted">
              {{
                t("events.detail", {
                  plugin: source.plugin_id,
                  state: state(source.state),
                  health: state(source.health),
                  reconnects: source.reconnects ?? 0,
                })
              }}
            </div>
            <div v-if="source.last_error" class="err-text">{{ source.last_error }}</div>
          </div>
          <MkButton size="sm" variant="ghost" @click="restart(source)">{{ t("events.restart") }}</MkButton>
        </div>
      </template>
    </MkAsync>
  </div>
</template>
