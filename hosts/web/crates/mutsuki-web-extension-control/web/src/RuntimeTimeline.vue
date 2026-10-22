<script setup lang="ts">
import { MkButton, MkCodeBlock, MkSearchInput, MkStatusBadge } from "@mutsuki/ui";
import { useMessages } from "@mutsuki/web-sdk";
import { computed, reactive, ref, watch } from "vue";

import { messages } from "./messages";
import type { TimelineGroup, TimelineRecord } from "./runtime-events";

/** Grouped event ledger with search, collapsible groups and an inspector for raw events. */
const props = defineProps<{
  records: TimelineRecord[];
  groups: TimelineGroup[];
  loading?: boolean;
  loadingOlder?: boolean;
  hasOlder?: boolean;
  lost?: number;
  truncated?: boolean;
  emptyText?: string;
  inspect?: (record: TimelineRecord) => Promise<unknown>;
}>();
const emit = defineEmits<{ older: [] }>();
const { t } = useMessages(messages);
const query = ref("");
const collapsed = reactive(new Set<string>());
const selectedId = ref<string | null>(null);
const raw = ref<{ loading: boolean; text: string }>({ loading: false, text: "" });

const visible = computed(() => {
  const needle = query.value.trim().toLowerCase();
  if (!needle) return props.records;
  return props.records.filter((record) =>
    [record.label, record.preview, record.input, record.output, record.groupLabel]
      .filter(Boolean)
      .join("\n")
      .toLowerCase()
      .includes(needle),
  );
});
const buckets = computed(() =>
  props.groups
    .map((group) => ({ group, records: visible.value.filter((record) => record.groupId === group.id) }))
    .filter((bucket) => bucket.records.length),
);
const status = computed(() =>
  [
    props.loading ? t("timeline.loading") : "",
    props.lost ? t("timeline.lost", { count: props.lost }) : "",
    props.truncated ? t("timeline.truncated") : "",
    t("timeline.count", { visible: visible.value.length, total: props.records.length }),
  ]
    .filter(Boolean)
    .join(" · "),
);
const selected = computed(() => props.records.find((record) => record.id === selectedId.value) ?? null);
watch(
  () => props.records,
  () => {
    if (selectedId.value && !selected.value) selectedId.value = null;
  },
);

async function select(record: TimelineRecord) {
  selectedId.value = record.id;
  raw.value = { loading: !!props.inspect, text: "" };
  if (!props.inspect) return;
  try {
    const detail = await props.inspect(record);
    if (selectedId.value !== record.id) return;
    raw.value = { loading: false, text: typeof detail === "string" ? detail : JSON.stringify(detail, null, 2) };
  } catch (error) {
    if (selectedId.value !== record.id) return;
    raw.value = { loading: false, text: error instanceof Error ? error.message : String(error) };
  }
}

function toggle(id: string) {
  if (collapsed.has(id)) collapsed.delete(id);
  else collapsed.add(id);
}
</script>

<template>
  <div class="trajectory">
    <div class="trajectory-toolbar">
      <MkButton v-if="hasOlder || loadingOlder" size="sm" :loading="loadingOlder" @click="emit('older')">
        {{ loadingOlder ? t("timeline.loadingOlder") : t("timeline.older") }}
      </MkButton>
      <MkSearchInput v-model="query" :label="t('timeline.search')" :debounce-ms="0" />
      <span class="muted" role="status">{{ status }}</span>
    </div>
    <div class="trajectory-body">
      <div class="trajectory-ledger" :aria-label="t('timeline.label')" role="group">
        <p v-if="!buckets.length" class="muted trajectory-empty">
          {{ loading ? t("timeline.loading") : (emptyText ?? t("timeline.empty")) }}
        </p>
        <template v-for="bucket in buckets" :key="bucket.group.id">
          <button
            type="button"
            class="trajectory-group"
            :aria-expanded="collapsed.has(bucket.group.id) ? 'false' : 'true'"
            @click="toggle(bucket.group.id)"
          >
            <span>{{ collapsed.has(bucket.group.id) ? "▸" : "▾" }} {{ bucket.group.label }}</span>
            <span class="muted">{{ bucket.records.length }}</span>
          </button>
          <template v-if="!collapsed.has(bucket.group.id)">
            <button
              v-for="record in bucket.records"
              :key="record.id"
              type="button"
              :class="[
                'trajectory-row',
                `is-${record.kind}`,
                { 'is-selected': record.id === selectedId, 'is-error': record.isError },
              ]"
              :aria-pressed="record.id === selectedId ? 'true' : 'false'"
              @click="select(record)"
            >
              <span class="trajectory-row__index">#{{ record.index }}</span>
              <span class="trajectory-row__kind">{{ record.label }}</span>
              <span class="trajectory-row__preview">{{ record.preview || "—" }}</span>
            </button>
          </template>
        </template>
      </div>
      <aside class="trajectory-inspector">
        <p v-if="!selected" class="muted">{{ t("timeline.inspect") }}</p>
        <template v-else>
          <h3>{{ selected.label }} <span class="muted">#{{ selected.index }}</span></h3>
          <p>
            {{ t("timeline.status") }}:
            <MkStatusBadge
              :tone="selected.isError ? 'err' : 'ok'"
              :label="selected.isError ? t('timeline.error') : t('timeline.done')"
            />
          </p>
          <template v-if="selected.input">
            <h4>{{ t("timeline.input") }}</h4>
            <MkCodeBlock :code="selected.input" :copyable="false" />
          </template>
          <template v-if="selected.output">
            <h4>{{ t("timeline.output") }}</h4>
            <MkCodeBlock :code="selected.output" :copyable="false" />
          </template>
          <template v-if="raw.loading || raw.text">
            <h4>{{ t("timeline.raw") }}</h4>
            <MkCodeBlock :code="raw.loading ? t('timeline.rawLoading') : raw.text" language="json" />
          </template>
        </template>
      </aside>
    </div>
  </div>
</template>
