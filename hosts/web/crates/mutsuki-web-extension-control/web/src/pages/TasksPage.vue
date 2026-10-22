<script setup lang="ts">
import { MkAsync, MkButton, MkCard, MkDataTable, MkEventLedger, MkKeyValue, type MkLedgerRecord } from "@mutsuki/ui";
import { callRpc, toRpcError, useMessages, useMutsukiRuntime } from "@mutsuki/web-sdk";
import { computed, onBeforeUnmount, ref, shallowRef, watch } from "vue";

import { useControlAction, useControlQuery } from "../composables";
import { controlApi, type RuntimeEvent, type TaskEntry } from "../contract";
import { messages } from "../messages";
import {
  EVENT_PAGE_LIMIT,
  EVENT_WINDOW_LIMIT,
  eventsOf,
  mergeBySequence,
  projectRuntimeEvents,
  sequenceOf,
  trimWindow,
  type TimelineRecord,
} from "../runtime-events";

const { t } = useMessages(messages);
const { client } = useMutsukiRuntime();
const tasks = useControlQuery(controlApi.task_list, ["tasks", "plugins"]);
const run = useControlAction();
const selectedId = ref<string | null>(null);

const selected = computed<TaskEntry | null>(() => {
  const list = tasks.data.value ?? [];
  return list.find((task) => task.task_id === selectedId.value) ?? list[0] ?? null;
});
const columns = computed(() => [
  { key: "task_id", label: t("tasks.task") },
  { key: "protocol_id", label: t("tasks.protocol") },
  { key: "status", label: t("tasks.status") },
  { key: "runner_hint", label: t("tasks.runner") },
]);
const detail = computed(() =>
  selected.value
    ? [
        { label: t("tasks.task"), value: selected.value.task_id, mono: true },
        { label: t("tasks.protocol"), value: selected.value.protocol_id },
        { label: t("tasks.status"), value: selected.value.status },
        { label: t("tasks.runner"), value: selected.value.owner_runner || selected.value.runner_hint },
      ]
    : [],
);

// Event window: newest page first, older pages on demand, bounded to EVENT_WINDOW_LIMIT.
const events = shallowRef<RuntimeEvent[]>([]);
const timeline = ref({ loading: false, loadingOlder: false, lost: 0, truncated: false, error: "" });
let earliest = 0;
let disposed = false;
onBeforeUnmount(() => (disposed = true));

const projection = computed(() => projectRuntimeEvents(events.value, { focusSubjectId: selected.value?.task_id }));
const firstSequence = () => {
  const values = events.value.map(sequenceOf).filter((value): value is number => value != null);
  return values.length ? Math.min(...values) : earliest + 1;
};
const hasOlder = computed(() => events.value.length > 0 && firstSequence() > (earliest || 1));

async function loadLatest() {
  timeline.value = { ...timeline.value, loading: true, error: "" };
  try {
    const probe = await callRpc(client, controlApi.task_events_after, { sequence: 0, limit: 1 });
    earliest = Number(probe.earliest_available_sequence ?? 0);
    const floor = Math.max(0, earliest ? earliest - 1 : 0);
    const latest = Number(probe.latest_sequence ?? 0);
    const page = await callRpc(client, controlApi.task_events_after, {
      sequence: Math.max(floor, latest - EVENT_PAGE_LIMIT),
      limit: EVENT_PAGE_LIMIT,
    });
    if (disposed) return;
    events.value = eventsOf(page);
    timeline.value = {
      loading: false,
      loadingOlder: false,
      lost: Number(page.lost ?? probe.lost ?? 0),
      truncated: !!(page.has_more || page.truncated),
      error: "",
    };
  } catch (error) {
    if (!disposed) timeline.value = { ...timeline.value, loading: false, error: toRpcError(error).message };
  }
}

async function loadOlder() {
  const first = firstSequence();
  if (first <= (earliest || 1)) return;
  timeline.value = { ...timeline.value, loadingOlder: true };
  try {
    const page = await callRpc(client, controlApi.task_events_after, {
      sequence: Math.max(Math.max(0, earliest ? earliest - 1 : 0), first - EVENT_PAGE_LIMIT - 1),
      limit: EVENT_PAGE_LIMIT,
    });
    if (disposed) return;
    const older = eventsOf(page).filter((event) => {
      const sequence = sequenceOf(event);
      return sequence != null && sequence < first;
    });
    events.value = trimWindow(mergeBySequence(events.value, older), EVENT_WINDOW_LIMIT, "older");
    timeline.value = { ...timeline.value, loadingOlder: false, lost: Number(page.lost ?? 0) };
  } catch (error) {
    if (!disposed) timeline.value = { ...timeline.value, loadingOlder: false, error: toRpcError(error).message };
  }
}

/** Raw events behind one record: its sequences are the runtime-event projection's own. */
async function inspect(record: MkLedgerRecord) {
  const sequences = (record as TimelineRecord).sequences ?? [];
  return events.value.filter((event) => sequences.includes(sequenceOf(event) ?? -1));
}

watch(() => tasks.data.value, () => void loadLatest(), { immediate: false });

function cancel(task: TaskEntry) {
  void run(controlApi.task_cancel, { id: task.task_id }, {
    confirm: { title: t("tasks.cancel"), message: t("tasks.cancelConfirm", { id: task.task_id }), danger: true },
    refresh: tasks.refresh,
  });
}
</script>

<template>
  <div class="page-body">
    <div class="toolbar row-item">
      <MkButton icon="refresh" :loading="tasks.loading.value" @click="tasks.refresh()">{{ t("refresh") }}</MkButton>
    </div>
    <div class="tasks-layout">
      <MkCard :title="t('tasks.table')">
        <MkAsync :source="tasks" :empty-title="t('tasks.empty')">
          <template #default="{ data }">
            <MkDataTable :caption="t('tasks.caption')" row-key="task_id" :columns="columns" :rows="data">
              <template #cell-task_id="{ row }">
                <button
                  type="button"
                  class="mk-focusable task-select mono"
                  :aria-pressed="row.task_id === selected?.task_id ? 'true' : 'false'"
                  @click="selectedId = String(row.task_id)"
                >
                  {{ row.task_id }}
                </button>
              </template>
            </MkDataTable>
          </template>
        </MkAsync>
      </MkCard>
      <MkCard :title="t('tasks.detail')">
        <p v-if="!selected" class="muted">{{ t("tasks.select") }}</p>
        <template v-else>
          <MkKeyValue :items="detail" />
          <MkButton variant="danger" size="sm" @click="cancel(selected)">{{ t("tasks.cancel") }}</MkButton>
        </template>
      </MkCard>
    </div>
    <MkCard :title="t('tasks.timeline')">
      <p class="muted">{{ t("tasks.timelineHint") }}</p>
      <p v-if="timeline.error" class="err-text" role="alert">{{ timeline.error }}</p>
      <MkEventLedger
        :records="projection.records"
        :groups="projection.groups"
        :loading="timeline.loading"
        :loading-older="timeline.loadingOlder"
        :has-older="hasOlder"
        :lost="timeline.lost"
        :truncated="timeline.truncated"
        :empty-text="t('tasks.timelineEmpty')"
        :inspect="inspect"
        @older="loadOlder"
      />
    </MkCard>
  </div>
</template>
