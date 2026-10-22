<script setup lang="ts">
import { MkAsync, MkButton, MkDataTable, MkDialog, MkField, MkStatusBadge, MkSwitch, MkTextInput, MkTextarea, useConfirm, useToast, type MkTone } from "@mutsuki/ui";
import { useMessages, useRpc } from "@mutsuki/web-sdk";
import { computed, reactive, ref } from "vue";

import { agentApi, type ConnectionConfig, type ConnectionState, type ConnectionStatus } from "./contract";
import { messages } from "./messages";
import { useAgentAction } from "./useAgentAction";

const { t } = useMessages(messages);
const confirm = useConfirm();
const toast = useToast();
const act = useAgentAction();
const snapshot = useRpc(agentApi["connections.snapshot"]);
const editor = reactive({ open: false, existing: false, connection_id: "", connector_id: "", enabled: true, config: "{}" });
const configError = ref<string | null>(null);

const TONE: Record<ConnectionState, MkTone> = { healthy: "ok", reconnecting: "warn", unavailable: "err", disabled: "neutral" };
const columns = computed(() => [
  { key: "connection_id", label: t("connections.id") },
  { key: "connector_id", label: t("connections.connector") },
  { key: "state", label: t("connections.state") },
  { key: "generation", label: t("connections.generation"), align: "end" as const },
  { key: "negotiated_version", label: t("connections.version") },
  { key: "last_error_code", label: t("connections.error") },
  { key: "actions", label: t("connections.actions") },
]);
const label = (id: string) => (id === "local" ? t("connections.local") : id);

function open(row?: ConnectionStatus) {
  Object.assign(editor, {
    open: true,
    existing: !!row,
    connection_id: row?.connection_id ?? "",
    connector_id: row?.connector_id ?? "",
    enabled: row ? row.state !== "disabled" : true,
    config: "{}",
  });
  configError.value = null;
}

/** The opaque connector config, or null (with a field error) when it is not a JSON object. */
function draft(): ConnectionConfig | null {
  try {
    const config = JSON.parse(editor.config);
    if (!config || typeof config !== "object" || Array.isArray(config)) throw new Error();
    configError.value = null;
    return { connection_id: editor.connection_id.trim(), connector_id: editor.connector_id.trim(), enabled: editor.enabled, config };
  } catch {
    configError.value = t("connections.configInvalid");
    return null;
  }
}

async function test() {
  const config = draft();
  if (!config) return;
  const status = await act(agentApi["connections.test"], { config });
  if (status) toast.info(t("connections.testOk", { state: t(`connections.states.${status.state}` as never) }));
}

async function save() {
  const config = draft();
  const revision = snapshot.data.value?.revision;
  if (!config || revision == null) return;
  if (!(await confirm({ title: t("connections.save"), message: t("connections.saveConfirm", { id: config.connection_id }) }))) return;
  if (await act(agentApi["connections.upsert"], { expected_revision: revision, config }, t("connections.saved"))) {
    editor.open = false;
  }
  await snapshot.refresh();
}

async function reconnect(row: ConnectionStatus) {
  const revision = snapshot.data.value?.revision;
  if (revision == null) return;
  if (!(await confirm({ title: t("connections.reconnect"), message: t("connections.reconnectConfirm", { id: label(row.connection_id) }) }))) return;
  await act(agentApi["connections.reconnect"], { expected_revision: revision, connection_id: row.connection_id });
  await snapshot.refresh();
}

function providerTest() {
  void act(agentApi["provider.test"], undefined, t("connections.providerOk"));
}
</script>

<template>
  <div class="page-body">
    <div class="toolbar row-item">
      <MkButton icon="refresh" :loading="snapshot.loading.value" @click="snapshot.refresh()">{{ t("refresh") }}</MkButton>
      <MkButton icon="plus" @click="open()">{{ t("connections.add") }}</MkButton>
      <MkButton @click="providerTest">{{ t("connections.providerTest") }}</MkButton>
    </div>
    <MkAsync :source="snapshot" :is-empty="(value) => !value.connections.length" :empty-title="t('connections.empty')">
      <template #default="{ data }">
        <MkDataTable :caption="t('connections.caption')" row-key="connection_id" :columns="columns" :rows="data.connections">
          <template #cell-connection_id="{ value }">{{ label(String(value)) }}</template>
          <template #cell-state="{ value }">
            <MkStatusBadge :tone="TONE[value as ConnectionState]" :label="t(`connections.states.${value}` as never)" />
          </template>
          <template #cell-actions="{ row }">
            <div class="row-actions">
              <MkButton size="sm" variant="ghost" @click="open(row as ConnectionStatus)">{{ t("connections.edit") }}</MkButton>
              <MkButton size="sm" @click="reconnect(row as ConnectionStatus)">{{ t("connections.reconnect") }}</MkButton>
            </div>
          </template>
        </MkDataTable>
      </template>
    </MkAsync>
    <MkDialog
      v-model:open="editor.open"
      :title="editor.existing ? t('connections.editTitle', { id: editor.connection_id }) : t('connections.addTitle')"
      size="lg"
    >
      <form class="mk-overlay__form" @submit.prevent="save">
        <MkField :label="t('connections.id')" required>
          <MkTextInput v-model="editor.connection_id" :disabled="editor.existing" />
        </MkField>
        <MkField :label="t('connections.connector')" required>
          <MkTextInput v-model="editor.connector_id" />
        </MkField>
        <MkField :label="t('connections.enabled')">
          <MkSwitch v-model="editor.enabled" />
        </MkField>
        <MkField :label="t('connections.config')" :hint="t('connections.configHint')" :error="configError">
          <MkTextarea v-model="editor.config" :rows="8" />
        </MkField>
      </form>
      <template #footer>
        <MkButton @click="test">{{ t("connections.test") }}</MkButton>
        <MkButton variant="primary" @click="save">{{ t("connections.save") }}</MkButton>
      </template>
    </MkDialog>
  </div>
</template>
