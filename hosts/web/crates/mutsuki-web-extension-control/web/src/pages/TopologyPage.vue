<script setup lang="ts">
import { MkAsync, MkButton } from "@mutsuki/ui";
import { callRpc, toRpcError, useMessages, useMutsukiRuntime, type RpcError } from "@mutsuki/web-sdk";
import { ref, shallowRef } from "vue";

import { usePolling } from "@mutsuki/web-sdk";

import { RECONCILE_MS, useDomainChanges, useStateLabel } from "../composables";
import { controlApi, type EventSourceEntry, type RunnerEntry } from "../contract";
import { messages } from "../messages";

interface Node {
  id: string;
  runners: RunnerEntry[];
  sources: EventSourceEntry[];
}

const { t } = useMessages(messages);
const { client } = useMutsukiRuntime();
const label = useStateLabel();
const data = shallowRef<Node[] | undefined>();
const error = ref<RpcError | null>(null);
const loading = ref(false);

async function refresh() {
  loading.value = true;
  try {
    const [plugins, runners, sources] = await Promise.all([
      callRpc(client, controlApi.plugin_list),
      callRpc(client, controlApi.runner_list),
      callRpc(client, controlApi.event_source_list),
    ]);
    const nodes = new Map<string, Node>();
    const node = (id: string) => {
      let entry = nodes.get(id);
      if (!entry) {
        entry = { id, runners: [], sources: [] };
        nodes.set(id, entry);
      }
      return entry;
    };
    for (const plugin of plugins.plugins ?? []) node(plugin.plugin_id);
    for (const runner of runners ?? []) node(runner.plugin_id).runners.push(runner);
    for (const source of sources ?? []) node(source.plugin_id).sources.push(source);
    data.value = [...nodes.values()];
    error.value = null;
  } catch (failure) {
    error.value = toRpcError(failure);
  } finally {
    loading.value = false;
  }
}

useDomainChanges(["plugins", "runners", "event_sources"], () => void refresh());
usePolling(refresh, RECONCILE_MS);
const source = { data, error, loading, refresh };
</script>

<template>
  <div class="page-body">
    <div class="toolbar row-item">
      <MkButton icon="refresh" :loading="loading" @click="refresh()">{{ t("refresh") }}</MkButton>
    </div>
    <MkAsync :source="source" :empty-title="t('topology.empty')">
      <template #default="{ data: nodes }">
        <div class="topology-graph">
          <section v-for="item in nodes" :key="item.id" class="topology-node card">
            <h2>{{ item.id }}</h2>
            <div class="topology-node__lanes">
              <div class="topology-lane">
                <h3>{{ t("topology.runners") }}</h3>
                <div v-for="runner in item.runners" :key="runner.runner_id" class="topology-chip">
                  <strong>{{ runner.runner_id }}</strong>
                  <span class="muted">{{ label(runner.state) }}</span>
                </div>
                <p v-if="!item.runners.length" class="muted">{{ t("topology.none") }}</p>
              </div>
              <div class="topology-lane">
                <h3>{{ t("topology.sources") }}</h3>
                <div v-for="entry in item.sources" :key="entry.source_id" class="topology-chip">
                  <strong>{{ entry.source_id }}</strong>
                  <span class="muted">{{ label(entry.health || entry.state) }}</span>
                </div>
                <p v-if="!item.sources.length" class="muted">{{ t("topology.none") }}</p>
              </div>
            </div>
          </section>
        </div>
      </template>
    </MkAsync>
  </div>
</template>
