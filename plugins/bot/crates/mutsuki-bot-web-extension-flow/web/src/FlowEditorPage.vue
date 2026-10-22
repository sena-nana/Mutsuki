<script setup lang="ts">
import { MkButton, MkErrorState, MkSkeleton, MkStatusBadge, useConfirm, useToast } from "@mutsuki/ui";
import { callRpc, toRpcError, useMessages, useMutsukiRuntime, useSession, type RpcError } from "@mutsuki/web-sdk";
import { computed, ref, shallowRef } from "vue";

import ContextMenu, { type MenuItem } from "./ContextMenu.vue";
import { flowApi, type FlowDocument, type NodeDescriptor, type ValidationResult } from "./contract";
import {
  canvasPorts,
  catalogNodes,
  compatibleTargets,
  connect,
  createNode,
  descriptorOf,
  emptyFlow,
  nodeMenu,
  portTitle,
  removeEdge,
  removeNode,
  type Endpoint,
} from "./flow-model";
import { NODE_WIDTH } from "./geometry";
import { messages } from "./messages";
import NodeCanvas, { type CanvasContextMenu } from "./NodeCanvas.vue";
import NodeInspector from "./NodeInspector.vue";

/**
 * The active Bot Flow as an editable graph. Saving validates first and applies against the
 * revision the graph was read at; a conflict keeps the edits until the user reloads.
 */
const { t } = useMessages(messages);
const { client } = useMutsukiRuntime();
const session = useSession();
const confirm = useConfirm();
const toast = useToast();

const catalog = shallowRef<NodeDescriptor[]>([]);
const revision = ref(0);
const flow = ref<FlowDocument>(emptyFlow());
const loaded = ref(false);
const loadError = ref<RpcError | null>(null);
const actionError = ref<{ key: "saveFailed" | "checkFailed"; error: RpcError } | null>(null);
const validation = ref<ValidationResult | null>(null);
const conflict = ref(false);
const dirty = ref(false);
const busy = ref<"check" | "save" | null>(null);
const notice = ref("");
const selectedNode = ref<string | null>(null);
const selectedEdge = ref<string | null>(null);
const menu = ref<{ x: number; y: number; items: MenuItem[]; searchable: boolean; label: string; onSelect: (item: MenuItem) => void } | null>(
  null,
);
const canvas = ref<InstanceType<typeof NodeCanvas> | null>(null);
const canWrite = computed(() => session.can("bot.flow.write"));

async function load() {
  loadError.value = null;
  try {
    const [nodes, snapshot] = await Promise.all([callRpc(client, flowApi["catalog.read"]), callRpc(client, flowApi["snapshot.read"])]);
    catalog.value = catalogNodes(nodes);
    revision.value = snapshot.revision;
    flow.value = structuredClone(snapshot.flow ?? emptyFlow());
    selectedNode.value = selectedEdge.value = null;
    validation.value = null;
    actionError.value = null;
    conflict.value = false;
    dirty.value = false;
    loaded.value = true;
  } catch (error) {
    loadError.value = toRpcError(error);
  }
}
void load();

const changed = () => {
  dirty.value = true;
  notice.value = "";
};

const invalidNodes = computed(() => new Set(validation.value?.issues.map((issue) => issue.node_id).filter(Boolean)));
const canvasNodes = computed(() =>
  flow.value.nodes.map((node) => {
    const desc = descriptorOf(catalog.value, node);
    return {
      id: node.node_id,
      x: node.position?.x ?? 40,
      y: node.position?.y ?? 40,
      title: desc?.title ?? node.node_type_id,
      subtitle: desc?.category ?? "",
      invalid: invalidNodes.value.has(node.node_id),
      inputs: canvasPorts(desc, "input"),
      outputs: canvasPorts(desc, "output"),
    };
  }),
);
const label = (nodeId: string, portId: string, direction: "input" | "output") => portTitle(flow.value, catalog.value, nodeId, portId, direction);
const canvasEdges = computed(() =>
  flow.value.edges.map((edge) => ({
    id: edge.edge_id,
    from: edge.from_node_id,
    fromPort: edge.from_port_id,
    to: edge.to_node_id,
    toPort: edge.to_port_id,
    kind: edge.kind,
    label: edge.kind === "error" ? "错误" : `${label(edge.from_node_id, edge.from_port_id, "output")} → ${label(edge.to_node_id, edge.to_port_id, "input")}`,
  })),
);
const node = computed(() => flow.value.nodes.find((item) => item.node_id === selectedNode.value) ?? null);
const edge = computed(() => flow.value.edges.find((item) => item.edge_id === selectedEdge.value) ?? null);
const nodeTitle = (nodeId: string) => canvasNodes.value.find((item) => item.id === nodeId)?.title ?? nodeId;

function select(nodeId: string | null, edgeId: string | null = null) {
  selectedNode.value = nodeId;
  selectedEdge.value = edgeId;
}

function addNode(desc: NodeDescriptor, position: { x: number; y: number }) {
  const created = createNode(desc, position);
  flow.value.nodes.push(created);
  select(created.node_id);
  changed();
}

function link(from: Endpoint, to: Endpoint) {
  const result = connect(flow.value, catalog.value, from, to);
  if (result.ok) changed();
  else if (result.reason === "incompatible") notice.value = t("incompatible");
}

function deleteSelection() {
  if (selectedEdge.value) removeEdge(flow.value, selectedEdge.value);
  else if (selectedNode.value) removeNode(flow.value, selectedNode.value);
  else return;
  select(null);
  changed();
}

function openAddMenu(x: number, y: number, at: { x: number; y: number }) {
  menu.value = {
    x,
    y,
    searchable: true,
    label: t("menu.nodes"),
    items: nodeMenu(catalog.value, (role) => t(`role.${role}`)),
    onSelect: (item) => item.descriptor && addNode(item.descriptor, at),
  };
}

function onContextMenu(event: CanvasContextMenu) {
  if (!canWrite.value) return;
  if (event.kind === "background") {
    openAddMenu(event.clientX, event.clientY, { x: event.world.x - NODE_WIDTH / 2, y: event.world.y - 24 });
    return;
  }
  const item = { id: "delete", label: t(event.kind === "edge" ? "menu.deleteEdge" : "menu.delete"), keywords: [], danger: true };
  menu.value = { x: event.clientX, y: event.clientY, searchable: false, label: t("menu.actions"), items: [item], onSelect: deleteSelection };
}

function onAddButton(event: MouseEvent) {
  const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
  const offset = flow.value.nodes.length * 28;
  openAddMenu(rect.left, rect.bottom + 4, { x: 48 + offset, y: 48 + offset });
}

function onMenuSelect(item: MenuItem) {
  const current = menu.value;
  menu.value = null;
  current?.onSelect(item);
}

function onMove(nodeId: string, x: number, y: number) {
  const target = flow.value.nodes.find((item) => item.node_id === nodeId);
  if (!target || (target.position.x === x && target.position.y === y)) return;
  target.position = { x, y };
  changed();
}

function onKeydown(event: KeyboardEvent) {
  const field = (event.target as HTMLElement).closest("input, textarea, select, [contenteditable]");
  if (event.key === "Escape") canvas.value?.cancelDraft();
  if ((event.key === "Delete" || event.key === "Backspace") && !field && canWrite.value) {
    event.preventDefault();
    deleteSelection();
  }
}

async function check(): Promise<boolean> {
  actionError.value = null;
  try {
    validation.value = await callRpc(client, flowApi.validate, { flow: flow.value });
    return validation.value.valid;
  } catch (error) {
    actionError.value = { key: "checkFailed", error: toRpcError(error) };
    return false;
  }
}

async function runCheck() {
  busy.value = "check";
  try {
    if (await check()) toast.success(t("valid"));
  } finally {
    busy.value = null;
  }
}

async function save() {
  busy.value = "save";
  try {
    if (!(await check())) return;
    await callRpc(client, flowApi.apply, { expected_revision: revision.value, flow: flow.value });
    toast.success(t("saved"));
    await load();
  } catch (error) {
    const failure = toRpcError(error);
    if (failure.code === "config.revision_conflict") conflict.value = true;
    else actionError.value = { key: "saveFailed", error: failure };
  } finally {
    busy.value = null;
  }
}

async function discard() {
  if (await confirm({ title: t("discard"), message: t("discardConfirm"), danger: true, confirmLabel: t("discard") })) await load();
}
</script>

<template>
  <div class="page-body">
  <MkErrorState v-if="loadError" :title="t('loadFailed')" :error="loadError" @retry="load" />
  <MkSkeleton v-else-if="!loaded" variant="block" />
  <div v-else class="lilia-node-editor lilia-workspace" data-lilia-surface-mode="solid" @keydown="onKeydown">
    <section class="lilia-workspace-region lilia-node-editor__stage" data-region="main" :aria-label="t('title')">
      <div class="lilia-workspace-region__content lilia-node-editor__stage-body">
        <div class="lilia-node-editor__toolbar">
          <MkStatusBadge :tone="dirty ? 'warn' : 'ok'" :label="dirty ? t('unsaved') : t('live')" />
          <MkButton size="sm" icon="plus" :disabled="!canWrite" @click="onAddButton">{{ t("addNode") }}</MkButton>
          <MkButton size="sm" variant="ghost" :loading="busy === 'check'" :disabled="busy !== null" @click="runCheck">{{ t("check") }}</MkButton>
          <MkButton size="sm" variant="ghost" :disabled="!dirty || busy !== null" @click="discard">{{ t("discard") }}</MkButton>
          <MkButton size="sm" variant="primary" :loading="busy === 'save'" :disabled="!canWrite || !dirty || busy !== null || conflict" @click="save">
            {{ t("save") }}
          </MkButton>
        </div>
        <div class="lilia-node-editor__issues" aria-live="polite">
          <div v-if="conflict" class="error-banner" role="alert">
            {{ t("conflict") }} <MkButton size="sm" @click="load">{{ t("reload") }}</MkButton>
          </div>
          <div v-if="actionError" class="error-banner" role="alert">
            {{ t(actionError.key, { message: actionError.error.message, code: actionError.error.code }) }}
          </div>
          <div v-if="notice" class="error-banner" role="alert">{{ notice }}</div>
          <div v-if="validation && !validation.valid" class="error-banner" role="alert">
            <p>{{ t("invalid", { count: validation.issues.length }) }}</p>
            <MkButton
              v-for="(issue, index) in validation.issues"
              :key="index"
              size="sm"
              variant="ghost"
              :disabled="!issue.node_id"
              @click="issue.node_id && select(issue.node_id)"
            >
              {{ issue.message }}
            </MkButton>
          </div>
        </div>
        <NodeCanvas
          ref="canvas"
          :nodes="canvasNodes"
          :edges="canvasEdges"
          :selected-node="selectedNode"
          :selected-edge="selectedEdge"
          :empty-text="t('empty')"
          @select-node="select($event)"
          @select-edge="select(null, $event)"
          @select-background="select(null)"
          @move="onMove"
          @connect="(from, fromPort, to, toPort) => link({ nodeId: from, portId: fromPort }, { nodeId: to, portId: toPort })"
          @invalid-connect="notice = t('incompatible')"
          @context-menu="onContextMenu"
        />
      </div>
    </section>
    <aside class="lilia-workspace-region lilia-node-editor__inspector" data-region="end" :aria-label="t('inspector.title')">
      <div class="lilia-workspace-region__content lilia-node-editor__pane">
        <h2 class="lilia-node-editor__pane-title">{{ t("inspector.title") }}</h2>
        <NodeInspector
          v-if="node"
          :key="node.node_id"
          :node="node"
          :descriptor="descriptorOf(catalog, node)"
          :outputs="canvasPorts(descriptorOf(catalog, node), 'output')"
          :targets="(portId) => compatibleTargets(flow, catalog, { nodeId: node!.node_id, portId })"
          @config="
            node.config = $event;
            changed();
          "
          @connect="(fromPort, to) => link({ nodeId: node!.node_id, portId: fromPort }, to)"
          @delete="deleteSelection"
        />
        <template v-else-if="edge">
          <p class="muted">
            {{
              t(edge.kind === "error" ? "inspector.errorEdge" : "inspector.edge", {
                from: nodeTitle(edge.from_node_id),
                to: nodeTitle(edge.to_node_id),
              })
            }}
          </p>
          <MkButton size="sm" variant="danger" :disabled="!canWrite" @click="deleteSelection">{{ t("inspector.deleteEdge") }}</MkButton>
        </template>
        <p v-else class="muted">{{ t("inspector.hint") }}</p>
      </div>
    </aside>
    <ContextMenu
      v-if="menu"
      :x="menu.x"
      :y="menu.y"
      :items="menu.items"
      :searchable="menu.searchable"
      :label="menu.label"
      @select="onMenuSelect"
      @close="menu = null"
    />
  </div>
  </div>
</template>
