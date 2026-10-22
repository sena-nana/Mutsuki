<script setup lang="ts" generic="Row extends object">
import { computed, ref } from "vue";
import { useUiText } from "../messages";
import type { MkColumn } from "../types";


const props = defineProps<{
  columns: MkColumn<Row>[];
  rows: Row[];
  /** Field holding a stable unique key. */
  rowKey: string;
  /** Accessible table name, rendered visually hidden. */
  caption: string;
  emptyText?: string;
}>();
const text = useUiText();
const sort = ref<{ key: string; direction: "ascending" | "descending" } | null>(null);

function field(row: Row, key: string): unknown {
  return (row as Record<string, unknown>)[key];
}

function cellText(row: Row, column: MkColumn<Row>): string {
  if (column.format) return column.format(row);
  const value = field(row, column.key);
  return value == null || value === "" ? "—" : String(value);
}

const sorted = computed(() => {
  const current = sort.value;
  if (!current) return props.rows;
  const column = props.columns.find((item) => item.key === current.key);
  if (!column) return props.rows;
  const factor = current.direction === "ascending" ? 1 : -1;
  return [...props.rows].sort((a, b) => {
    const left = field(a, column.key);
    const right = field(b, column.key);
    if (typeof left === "number" && typeof right === "number") return (left - right) * factor;
    return cellText(a, column).localeCompare(cellText(b, column), "zh-CN") * factor;
  });
});

function toggleSort(key: string) {
  const current = sort.value;
  sort.value =
    current?.key === key && current.direction === "ascending"
      ? { key, direction: "descending" }
      : { key, direction: "ascending" };
}
</script>

<template>
  <div class="mk-table-wrap">
    <table class="mk-table">
      <caption class="mk-visually-hidden">{{ caption }}</caption>
      <thead>
        <tr>
          <th
            v-for="column in columns"
            :key="column.key"
            scope="col"
            :class="`mk-table__cell--${column.align ?? 'start'}`"
            :aria-sort="sort?.key === column.key ? sort.direction : undefined"
          >
            <button
              v-if="column.sortable"
              type="button"
              class="mk-table__sort mk-focusable"
              @click="toggleSort(column.key)"
            >
              {{ column.label }}
              <span aria-hidden="true">{{
                sort?.key === column.key ? (sort.direction === "ascending" ? "↑" : "↓") : "↕"
              }}</span>
            </button>
            <template v-else>{{ column.label }}</template>
          </th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="row in sorted" :key="String(field(row, rowKey))">
          <td
            v-for="column in columns"
            :key="column.key"
            :class="`mk-table__cell--${column.align ?? 'start'}`"
          >
            <slot :name="`cell-${column.key}`" :row="row" :value="field(row, column.key)">
              {{ cellText(row, column) }}
            </slot>
          </td>
        </tr>
        <tr v-if="!rows.length">
          <td class="mk-table__empty" :colspan="columns.length">{{ emptyText ?? text("empty") }}</td>
        </tr>
      </tbody>
    </table>
  </div>
</template>

<style>
.mk-table-wrap {
  overflow-x: auto;
  border: 1px solid var(--mk-color-border);
  border-radius: var(--mk-radius-lg);
}
.mk-table {
  width: 100%;
  border-collapse: collapse;
  font-size: var(--mk-font-size);
}
.mk-table th,
.mk-table td {
  padding: var(--mk-space-2) var(--mk-space-3);
  border-bottom: 1px solid var(--mk-color-border-soft);
  vertical-align: top;
}
.mk-table tbody tr:last-child td {
  border-bottom: 0;
}
.mk-table th {
  background: var(--mk-color-subtle);
  color: var(--mk-color-text-muted);
  font-weight: 500;
  text-align: start;
  white-space: nowrap;
}
.mk-table tbody tr:hover {
  background: var(--mk-state-hover);
}
.mk-table__cell--end {
  text-align: end;
}
.mk-table__cell--center {
  text-align: center;
}
.mk-table__sort {
  display: inline-flex;
  gap: var(--mk-space-1);
  padding: 0;
  border: 0;
  background: none;
  color: inherit;
  font: inherit;
  cursor: pointer;
}
.mk-table__empty {
  padding: var(--mk-space-5);
  color: var(--mk-color-text-muted);
  text-align: center;
}
</style>
