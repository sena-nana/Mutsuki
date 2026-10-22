<script setup lang="ts">
import { formatDuration } from "@mutsuki/web-sdk";
import { computed, reactive, ref, watch } from "vue";

import { useUiText } from "../messages";
import type { MkLedgerGroup, MkLedgerRecord } from "../types";
import MkButton from "./MkButton.vue";
import MkCodeBlock from "./MkCodeBlock.vue";
import MkSearchInput from "./MkSearchInput.vue";
import MkStatusBadge from "./MkStatusBadge.vue";

/**
 * Grouped event ledger (run/session timelines) with search, collapsible groups, older-page
 * loading and an inspector that can fetch the raw events behind a record.
 */
const props = defineProps<{
  records: MkLedgerRecord[];
  groups: MkLedgerGroup[];
  loading?: boolean;
  loadingOlder?: boolean;
  hasOlder?: boolean;
  lost?: number;
  truncated?: boolean;
  emptyText?: string;
  inspect?: (record: MkLedgerRecord) => Promise<unknown>;
}>();
const emit = defineEmits<{ older: [] }>();
const t = useUiText();
const query = ref("");
const collapsed = reactive(new Set<string>());
const selectedId = ref<string | null>(null);
const raw = ref<{ loading: boolean; text: string }>({ loading: false, text: "" });

const visible = computed(() => {
  const needle = query.value.trim().toLowerCase();
  if (!needle) return props.records;
  return props.records.filter((record) =>
    [record.label, record.preview, record.input, record.output, record.thinking]
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
    props.loading ? t("ledger.loading") : "",
    props.lost ? t("ledger.lost", { count: props.lost }) : "",
    props.truncated ? t("ledger.truncated") : "",
    t("ledger.count", { visible: visible.value.length, total: props.records.length }),
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

async function select(record: MkLedgerRecord) {
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
        {{ loadingOlder ? t("ledger.loadingOlder") : t("ledger.older") }}
      </MkButton>
      <MkSearchInput v-model="query" :label="t('ledger.search')" :debounce-ms="0" />
      <span class="muted" role="status">{{ status }}</span>
    </div>
    <div class="trajectory-body">
      <div class="trajectory-ledger" :aria-label="t('ledger.label')" role="group">
        <p v-if="!buckets.length" class="muted trajectory-empty">
          {{ loading ? t("ledger.loading") : (emptyText ?? t("ledger.empty")) }}
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
                {
                  'is-selected': record.id === selectedId,
                  'is-error': record.isError,
                  'is-running': record.status === 'running',
                  'is-nested': !!record.parentId,
                },
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
        <p v-if="!selected" class="muted">{{ t("ledger.inspect") }}</p>
        <template v-else>
          <h3>{{ selected.label }} <span class="muted">#{{ selected.index }}</span></h3>
          <p>
            {{ t("ledger.status") }}:
            <MkStatusBadge
              :tone="selected.isError ? 'err' : selected.status === 'running' ? 'info' : 'ok'"
              :label="
                selected.isError
                  ? t('ledger.error')
                  : selected.status === 'running'
                    ? t('ledger.running')
                    : t('ledger.done')
              "
            />
            <span v-if="selected.durationMs != null" class="muted">
              {{ t("ledger.duration", { duration: formatDuration(selected.durationMs) }) }}
            </span>
          </p>
          <template v-if="selected.input">
            <h4>{{ t("ledger.input") }}</h4>
            <MkCodeBlock :code="selected.input" :copyable="false" />
          </template>
          <template v-if="selected.thinking">
            <h4>{{ t("ledger.thinking") }}</h4>
            <MkCodeBlock :code="selected.thinking" :copyable="false" />
          </template>
          <template v-if="selected.output">
            <h4>{{ t("ledger.output") }}</h4>
            <MkCodeBlock :code="selected.output" :copyable="false" />
          </template>
          <template v-if="raw.loading || raw.text">
            <h4>{{ t("ledger.raw") }}</h4>
            <MkCodeBlock :code="raw.loading ? t('ledger.rawLoading') : raw.text" language="json" />
          </template>
        </template>
      </aside>
    </div>
  </div>
</template>
