<script setup lang="ts">
import { computed, nextTick, onMounted, ref, shallowRef, watch } from "vue";

import type { TypeRef } from "./contract";
import type { CanvasPort } from "./flow-model";
import { clientToWorld, edgePath, portAnchor, samePortType, zoomAt, type Point, type View } from "./geometry";

export interface CanvasNode {
  id: string;
  x: number;
  y: number;
  title: string;
  subtitle: string;
  invalid: boolean;
  inputs: CanvasPort[];
  outputs: CanvasPort[];
}

export interface CanvasEdge {
  id: string;
  from: string;
  fromPort: string;
  to: string;
  toPort: string;
  kind: "event" | "error";
  label: string;
}

export interface CanvasContextMenu {
  kind: "node" | "edge" | "background";
  id: string | null;
  clientX: number;
  clientY: number;
  world: Point;
}

/**
 * Pan (drag the background), zoom (Ctrl/⌘ + wheel), move nodes and wire an output port to a
 * compatible input port. The canvas owns no flow state: it reports intents to its parent.
 */
const props = defineProps<{
  nodes: CanvasNode[];
  edges: CanvasEdge[];
  selectedNode: string | null;
  selectedEdge: string | null;
  emptyText: string;
}>();
const emit = defineEmits<{
  selectNode: [id: string];
  selectEdge: [id: string];
  selectBackground: [];
  move: [id: string, x: number, y: number];
  connect: [from: string, fromPort: string, to: string, toPort: string];
  invalidConnect: [];
  contextMenu: [menu: CanvasContextMenu];
}>();

const host = ref<HTMLElement | null>(null);
const world = ref<HTMLElement | null>(null);
const view = ref<View>({ x: 24, y: 24, scale: 1 });
const dragging = ref<{ id: string; dx: number; dy: number; x: number; y: number; pointerId: number } | null>(null);
const panning = ref<{ x: number; y: number; viewX: number; viewY: number; pointerId: number } | null>(null);
const draft = ref<{ from: string; fromPort: string; type: TypeRef; start: Point; end: Point; pointerId: number } | null>(null);
/** Port centers relative to their node, measured once the cards are laid out. */
const offsets = shallowRef(new Map<string, Point>());

const portKey = (node: string, direction: string, port: string) => `${node}|${direction}|${port}`;
const position = (node: CanvasNode): Point => (dragging.value?.id === node.id ? dragging.value : node);

function anchor(nodeId: string, direction: "input" | "output", portId: string): Point | null {
  const node = props.nodes.find((item) => item.id === nodeId);
  if (!node) return null;
  const origin = position(node);
  const measured = offsets.value.get(portKey(nodeId, direction, portId));
  if (measured) return { x: origin.x + measured.x, y: origin.y + measured.y };
  const ports = direction === "output" ? node.outputs : node.inputs;
  return portAnchor(origin, direction, Math.max(0, ports.findIndex((port) => port.port_id === portId)));
}

const wires = computed(() =>
  props.edges.flatMap((edge) => {
    const from = anchor(edge.from, "output", edge.fromPort);
    const to = anchor(edge.to, "input", edge.toPort);
    return from && to ? [{ ...edge, d: edgePath(from, to) }] : [];
  }),
);
const draftPath = computed(() => (draft.value ? edgePath(draft.value.start, draft.value.end) : ""));

function worldPoint(event: { clientX: number; clientY: number }): Point {
  return clientToWorld(world.value!.getBoundingClientRect(), view.value, event.clientX, event.clientY);
}

function measure() {
  if (!world.value) return;
  const next = new Map<string, Point>();
  for (const card of world.value.querySelectorAll<HTMLElement>("[data-node]")) {
    const origin = card.getBoundingClientRect();
    if (!origin.width) continue;
    for (const dot of card.querySelectorAll<HTMLElement>("[data-port] .lilia-node-editor__port-dot")) {
      const port = dot.closest<HTMLElement>("[data-port]")!;
      const rect = dot.getBoundingClientRect();
      next.set(portKey(card.dataset.node!, port.dataset.direction!, port.dataset.port!), {
        x: (rect.left + rect.width / 2 - origin.left) / view.value.scale,
        y: (rect.top + rect.height / 2 - origin.top) / view.value.scale,
      });
    }
  }
  offsets.value = next;
}

onMounted(measure);
watch(
  () => props.nodes.map((node) => `${node.id}:${node.inputs.length}:${node.outputs.length}:${node.title}`).join(),
  () => void nextTick(measure),
);

function onBackgroundDown(event: PointerEvent) {
  if ((event.target as Element).closest("[data-node], [data-edge]") || (event.button !== 0 && event.button !== 1)) return;
  panning.value = { x: event.clientX, y: event.clientY, viewX: view.value.x, viewY: view.value.y, pointerId: event.pointerId };
  host.value?.setPointerCapture?.(event.pointerId);
  emit("selectBackground");
}

function onNodeDown(event: PointerEvent, node: CanvasNode) {
  if (event.button !== 0) return;
  emit("selectNode", node.id);
  const point = worldPoint(event);
  dragging.value = { id: node.id, dx: point.x - node.x, dy: point.y - node.y, x: node.x, y: node.y, pointerId: event.pointerId };
  host.value?.setPointerCapture?.(event.pointerId);
}

function finishConnect(nodeId: string, portId: string) {
  const current = draft.value;
  draft.value = null;
  if (!current || current.from === nodeId) return;
  const target = props.nodes.find((node) => node.id === nodeId)?.inputs.find((port) => port.port_id === portId);
  if (!samePortType(current.type, target?.event_type)) emit("invalidConnect");
  else emit("connect", current.from, current.fromPort, nodeId, portId);
}

function onPortDown(event: PointerEvent, node: CanvasNode, port: CanvasPort, direction: "input" | "output") {
  if (event.button !== 0) return;
  if (direction === "input") {
    if (draft.value) finishConnect(node.id, port.port_id);
    return;
  }
  const start = anchor(node.id, "output", port.port_id)!;
  draft.value = { from: node.id, fromPort: port.port_id, type: port.event_type, start, end: start, pointerId: event.pointerId };
  host.value?.setPointerCapture?.(event.pointerId);
}

function onPointerMove(event: PointerEvent) {
  if (panning.value) {
    view.value = { ...view.value, x: panning.value.viewX + event.clientX - panning.value.x, y: panning.value.viewY + event.clientY - panning.value.y };
  } else if (dragging.value) {
    const point = worldPoint(event);
    dragging.value = { ...dragging.value, x: Math.max(0, point.x - dragging.value.dx), y: Math.max(0, point.y - dragging.value.dy) };
  } else if (draft.value) {
    draft.value = { ...draft.value, end: worldPoint(event) };
  }
}

function onPointerUp(event: PointerEvent) {
  if (panning.value?.pointerId === event.pointerId) {
    panning.value = null;
  } else if (dragging.value?.pointerId === event.pointerId) {
    const { id, x, y } = dragging.value;
    dragging.value = null;
    emit("move", id, Math.round(x), Math.round(y));
  } else if (draft.value?.pointerId === event.pointerId) {
    const target = document.elementFromPoint?.(event.clientX, event.clientY)?.closest<HTMLElement>('[data-port][data-direction="input"]');
    const nodeId = target?.closest<HTMLElement>("[data-node]")?.dataset.node;
    if (target && nodeId) finishConnect(nodeId, target.dataset.port!);
    else if (Math.hypot(draft.value.end.x - draft.value.start.x, draft.value.end.y - draft.value.start.y) > 8) draft.value = null;
  }
}

function onContextMenu(event: MouseEvent) {
  draft.value = null;
  const node = (event.target as Element).closest<HTMLElement>("[data-node]")?.dataset.node ?? null;
  const edge = (event.target as Element).closest<SVGElement>("[data-edge]")?.dataset.edge ?? null;
  if (node) emit("selectNode", node);
  else if (edge) emit("selectEdge", edge);
  emit("contextMenu", { kind: node ? "node" : edge ? "edge" : "background", id: node ?? edge, clientX: event.clientX, clientY: event.clientY, world: worldPoint(event) });
}

function onWheel(event: WheelEvent) {
  if (!event.ctrlKey && !event.metaKey) return;
  event.preventDefault();
  view.value = zoomAt(view.value, world.value!.getBoundingClientRect(), event.clientX, event.clientY, event.deltaY < 0);
}

/** Drops an in-progress wire (Escape). */
function cancelDraft() {
  draft.value = null;
}

defineExpose({ cancelDraft, clientToWorld: (clientX: number, clientY: number) => worldPoint({ clientX, clientY }) });
</script>

<template>
  <div
    ref="host"
    class="lilia-node-editor__viewport"
    :class="{ 'is-panning': panning, 'is-wiring': draft }"
    :style="{ '--lilia-node-editor-grid-x': `${view.x}px`, '--lilia-node-editor-grid-y': `${view.y}px` }"
    @pointerdown="onBackgroundDown"
    @pointermove="onPointerMove"
    @pointerup="onPointerUp"
    @pointercancel="onPointerUp"
    @contextmenu.prevent="onContextMenu"
    @wheel="onWheel"
  >
    <div ref="world" class="lilia-node-editor__world" :style="{ transform: `translate(${view.x}px, ${view.y}px) scale(${view.scale})` }">
      <svg class="lilia-node-editor__wires" aria-hidden="true">
        <path
          v-for="wire in wires"
          :key="wire.id"
          :class="['lilia-node-editor__wire', { error: wire.kind === 'error', 'is-selected': wire.id === selectedEdge }]"
          :data-edge="wire.id"
          :d="wire.d"
          style="pointer-events: stroke"
          @pointerdown.stop="$event.button === 0 && emit('selectEdge', wire.id)"
        >
          <title>{{ wire.label }}</title>
        </path>
        <path v-if="draft" class="lilia-node-editor__wire is-draft" :d="draftPath" />
      </svg>
      <div class="lilia-node-editor__nodes">
        <article
          v-for="node in nodes"
          :key="node.id"
          :data-node="node.id"
          :class="[
            'lilia-node-editor__node',
            { 'is-selected': node.id === selectedNode, 'is-invalid': node.invalid, 'is-dragging': dragging?.id === node.id },
          ]"
          :style="{ left: 0, top: 0, transform: `translate(${position(node).x}px, ${position(node).y}px)` }"
          :aria-label="node.title"
          tabindex="0"
          @pointerdown.stop="onNodeDown($event, node)"
          @keydown.enter.self="emit('selectNode', node.id)"
        >
          <h3 class="lilia-node-editor__node-title">{{ node.title }}</h3>
          <p class="lilia-node-editor__node-meta">{{ node.subtitle }}</p>
          <div class="lilia-node-editor__ports">
            <div class="lilia-node-editor__port-col lilia-node-editor__port-col--input">
              <button
                v-for="port in node.inputs"
                :key="port.port_id"
                type="button"
                class="lilia-node-editor__port input"
                :data-port="port.port_id"
                data-direction="input"
                :title="port.title"
                @pointerdown.stop="onPortDown($event, node, port, 'input')"
              >
                <span class="lilia-node-editor__port-dot" />
                <span class="lilia-node-editor__port-label">{{ port.title }}</span>
              </button>
            </div>
            <div class="lilia-node-editor__port-col lilia-node-editor__port-col--output">
              <button
                v-for="port in node.outputs"
                :key="port.port_id"
                type="button"
                :class="['lilia-node-editor__port output', { error: port.kind === 'error', 'is-active': draft?.from === node.id && draft?.fromPort === port.port_id }]"
                :data-port="port.port_id"
                data-direction="output"
                :title="port.title"
                @pointerdown.stop="onPortDown($event, node, port, 'output')"
              >
                <span class="lilia-node-editor__port-dot" />
                <span class="lilia-node-editor__port-label">{{ port.title }}</span>
              </button>
            </div>
          </div>
        </article>
      </div>
    </div>
    <p v-if="!nodes.length" class="lilia-node-editor__empty muted">{{ emptyText }}</p>
  </div>
</template>
