<script setup lang="ts">
import { MkAsync, MkButton } from "@mutsuki/ui";
import { useMessages } from "@mutsuki/web-sdk";

import { useControlAction, useControlQuery, useStateLabel } from "../composables";
import { controlApi, type RunnerEntry } from "../contract";
import { messages } from "../messages";

const { t } = useMessages(messages);
const runners = useControlQuery(controlApi.runner_list, ["runners"]);
const run = useControlAction();
const state = useStateLabel();

function restart(runner: RunnerEntry) {
  void run(controlApi.runner_restart, { id: runner.runner_id }, {
    confirm: { title: t("runners.restart"), message: t("runners.restartConfirm", { id: runner.runner_id }) },
    refresh: runners.refresh,
  });
}

function stop(runner: RunnerEntry) {
  void run(controlApi.runner_stop, { id: runner.runner_id }, {
    confirm: { title: t("runners.stop"), message: t("runners.stopConfirm", { id: runner.runner_id }), danger: true },
    refresh: runners.refresh,
  });
}
</script>

<template>
  <div class="page-body">
    <div class="toolbar row-item">
      <MkButton icon="refresh" :loading="runners.loading.value" @click="runners.refresh()">{{ t("refresh") }}</MkButton>
    </div>
    <MkAsync :source="runners" :empty-title="t('runners.empty')">
      <template #default="{ data }">
        <div v-for="runner in data" :key="runner.runner_id" class="tree-item row-item">
          <div>
            <strong>{{ runner.runner_id }}</strong>
            <div class="muted">
              {{
                t("runners.detail", {
                  plugin: runner.plugin_id,
                  state: state(runner.state),
                  pid: runner.pid ?? "—",
                  restarts: runner.restarts ?? 0,
                })
              }}
            </div>
          </div>
          <div class="row-actions">
            <MkButton size="sm" variant="ghost" @click="restart(runner)">{{ t("runners.restart") }}</MkButton>
            <MkButton size="sm" variant="danger" @click="stop(runner)">{{ t("runners.stop") }}</MkButton>
          </div>
        </div>
      </template>
    </MkAsync>
  </div>
</template>
