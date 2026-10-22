<script setup lang="ts">
import { MkAsync, MkButton, MkCard, MkDataTable, MkEmptyState, MkErrorState, MkSkeleton, MkSplitPane } from "@mutsuki/ui";
import { formatBytes, useMessages, useRpc } from "@mutsuki/web-sdk";
import { computed, ref, watch } from "vue";

import { formatCell } from "./cells";
import { PAGE_SIZE, databaseApi, type DatabaseSnapshot } from "./contract";
import { messages } from "./messages";

const { t } = useMessages(messages);
const snapshot = useRpc(databaseApi.snapshot);
const selected = ref("");
// Cursor that starts each visited page; index 0 is the beginning of the table.
const cursors = ref<string[]>([""]);
const pageIndex = ref(0);
const rows = useRpc(
  databaseApi.rows,
  () => ({ table: selected.value, after: cursors.value[pageIndex.value] ?? "", limit: PAGE_SIZE }),
  { immediate: false },
);
const split = ref(28);

watch(
  () => snapshot.data.value,
  (value) => {
    // A table that disappeared (migration, reload) is deselected rather than left dangling.
    if (selected.value && !value?.tables.some((table) => table.name === selected.value)) select("");
  },
);

function select(name: string) {
  selected.value = name;
  cursors.value = [""];
  pageIndex.value = 0;
  if (name) void rows.refresh();
}

function next() {
  const cursor = rows.data.value?.next_cursor;
  if (!cursor) return;
  cursors.value[pageIndex.value + 1] = cursor;
  pageIndex.value += 1;
}

function previous() {
  if (pageIndex.value > 0) pageIndex.value -= 1;
}

function refresh() {
  void snapshot.refresh();
  if (selected.value) void rows.refresh();
}

const table = computed(() => snapshot.data.value?.tables.find((item) => item.name === selected.value));
const columns = computed(() =>
  (rows.data.value?.columns ?? []).map((name) => {
    const field = table.value?.columns.find((column) => column.name === name);
    return {
      key: name,
      label: field?.primary_key ? `${name} *` : name,
      format: (row: Record<string, unknown>) => formatCell(row[name] as never).text,
    };
  }),
);
const tableRows = computed(() =>
  (rows.data.value?.rows ?? []).map((cells, index) => {
    const row: Record<string, unknown> = { __row: `${pageIndex.value}:${index}` };
    (rows.data.value?.columns ?? []).forEach((name, column) => (row[name] = cells[column]));
    return row;
  }),
);
const isUnbound = (value: DatabaseSnapshot | null) => value === null;
</script>

<template>
  <div class="page-body database-page">
    <div class="toolbar row-item">
      <MkButton icon="refresh" :loading="snapshot.loading.value" @click="refresh">{{ t("refresh") }}</MkButton>
      <span v-if="snapshot.data.value" class="muted">
        {{ t("size", { size: formatBytes(snapshot.data.value.size_bytes) }) }}
      </span>
    </div>
    <MkAsync :source="snapshot" :is-empty="isUnbound" :empty-title="t('unbound')">
      <template #default="{ data }">
        <MkSplitPane v-model="split" :min="18" :max="45">
          <template #start>
            <MkCard :title="t('tables')">
              <p v-if="!data!.tables.length" class="muted">{{ t("noTables") }}</p>
              <div v-else class="database-tables">
                <button
                    v-for="item in data!.tables"
                    :key="item.name"
                    type="button"
                    class="tree-item mk-focusable database-table"
                    :aria-pressed="item.name === selected ? 'true' : 'false'"
                    @click="select(item.name)"
                  >
                    <strong>{{ item.name }}</strong>
                    <span class="muted">
                      {{ t("tableMeta", { rows: item.row_count, columns: item.columns.length }) }}
                    </span>
                  </button>
              </div>
            </MkCard>
          </template>
          <template #end>
            <MkCard :title="selected || t('title')">
              <MkEmptyState v-if="!selected" :title="t('pick')" />
              <MkErrorState v-else-if="rows.error.value" :error="rows.error.value" @retry="rows.refresh()" />
              <MkSkeleton v-else-if="!rows.data.value" />
              <template v-else>
                <MkDataTable
                  :caption="t('caption', { table: selected })"
                  row-key="__row"
                  :columns="columns"
                  :rows="tableRows"
                  :empty-text="t('empty')"
                />
                <div class="toolbar row-item">
                  <MkButton size="sm" :disabled="pageIndex === 0" @click="previous">{{ t("previous") }}</MkButton>
                  <MkButton size="sm" :disabled="!rows.data.value.next_cursor" @click="next">{{ t("next") }}</MkButton>
                  <span class="muted">{{ t("page", { page: pageIndex + 1, rows: tableRows.length }) }}</span>
                </div>
              </template>
            </MkCard>
          </template>
        </MkSplitPane>
      </template>
    </MkAsync>
  </div>
</template>
