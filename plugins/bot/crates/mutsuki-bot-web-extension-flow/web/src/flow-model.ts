import type { FlowDocument, FlowEdge, FlowNode, NodeDescriptor, NodeRole, TypeRef } from "./contract";
import { ERROR_TYPE, samePortType } from "./geometry";

export const ROLE_ORDER: NodeRole[] = ["source", "match", "processor", "sink"];

export const emptyFlow = (): FlowDocument => ({ flow_id: "default", name: "流程", nodes: [], edges: [] });

export const catalogNodes = (body: NodeDescriptor[] | { nodes: NodeDescriptor[] } | null | undefined): NodeDescriptor[] =>
  Array.isArray(body) ? body : (body?.nodes ?? []);

export function descriptorOf(catalog: NodeDescriptor[], node: FlowNode | null | undefined): NodeDescriptor | undefined {
  return node ? catalog.find((item) => item.node_type_id === node.node_type_id && item.version === node.node_type_version) : undefined;
}

export function defaultConfig(desc: NodeDescriptor | undefined): Record<string, unknown> {
  const config: Record<string, unknown> = {};
  for (const [key, schema] of Object.entries(desc?.config_schema?.properties ?? {})) {
    if (schema.default !== undefined) config[key] = structuredClone(schema.default);
    else if (schema.type === "array") config[key] = [];
    else if (schema.type === "boolean") config[key] = false;
  }
  return config;
}

/** Source nodes subscribe to the ingest protocol for their output event type. */
export function sourceSelector(desc: NodeDescriptor | undefined): FlowNode["source"] {
  const eventType = desc?.ports.find((port) => port.direction === "output")?.event_type;
  return {
    protocol_id: "mutsuki.bot.event/ingest@1",
    event_type: eventType ? { type_id: eventType.type_id, version: eventType.version } : null,
  };
}

export const newId = (prefix: string) => `${prefix}-${crypto.randomUUID()}`;

export function createNode(desc: NodeDescriptor, position: { x: number; y: number }): FlowNode {
  return {
    node_id: newId("node"),
    node_type_id: desc.node_type_id,
    node_type_version: desc.version,
    config: defaultConfig(desc),
    source: desc.role === "source" ? sourceSelector(desc) : null,
    position: { x: Math.max(0, Math.round(position.x)), y: Math.max(0, Math.round(position.y)) },
  };
}

export interface CanvasPort {
  port_id: string;
  title: string;
  kind: "event" | "error";
  event_type: TypeRef;
}

/** Ports as the canvas draws them: every non-source node also has an `error` output. */
export function canvasPorts(desc: NodeDescriptor | undefined, direction: "input" | "output"): CanvasPort[] {
  const ports: CanvasPort[] = (desc?.ports ?? [])
    .filter((port) => port.direction === direction)
    .map((port) => ({ port_id: port.port_id, title: port.title, kind: "event", event_type: port.event_type }));
  if (direction === "output" && desc && desc.role !== "source") {
    ports.push({ port_id: "error", title: "错误", kind: "error", event_type: ERROR_TYPE });
  }
  return ports;
}

export interface Endpoint {
  nodeId: string;
  portId: string;
}

/** The type flowing out of an output port, or into an input port. */
export function portType(flow: FlowDocument, catalog: NodeDescriptor[], end: Endpoint, direction: "input" | "output"): TypeRef | undefined {
  if (direction === "output" && end.portId === "error") return ERROR_TYPE;
  const node = flow.nodes.find((item) => item.node_id === end.nodeId);
  return descriptorOf(catalog, node)?.ports.find((port) => port.port_id === end.portId && port.direction === direction)?.event_type;
}

export type ConnectResult = { ok: true; edge: FlowEdge } | { ok: false; reason: "incompatible" | "duplicate" | "self" };

/** Adds an edge when the port types match; the caller reports a refusal. */
export function connect(flow: FlowDocument, catalog: NodeDescriptor[], from: Endpoint, to: Endpoint): ConnectResult {
  if (from.nodeId === to.nodeId) return { ok: false, reason: "self" };
  if (!samePortType(portType(flow, catalog, from, "output"), portType(flow, catalog, to, "input"))) return { ok: false, reason: "incompatible" };
  const duplicate = flow.edges.some(
    (edge) =>
      edge.from_node_id === from.nodeId && edge.from_port_id === from.portId && edge.to_node_id === to.nodeId && edge.to_port_id === to.portId,
  );
  if (duplicate) return { ok: false, reason: "duplicate" };
  const edge: FlowEdge = {
    edge_id: newId("edge"),
    from_node_id: from.nodeId,
    from_port_id: from.portId,
    to_node_id: to.nodeId,
    to_port_id: to.portId,
    kind: from.portId === "error" ? "error" : "event",
  };
  flow.edges.push(edge);
  return { ok: true, edge };
}

/** Every input port a given output may connect to. */
export function compatibleTargets(flow: FlowDocument, catalog: NodeDescriptor[], from: Endpoint): Array<Endpoint & { label: string }> {
  const source = portType(flow, catalog, from, "output");
  return flow.nodes
    .filter((node) => node.node_id !== from.nodeId)
    .flatMap((node) => {
      const desc = descriptorOf(catalog, node);
      return (desc?.ports ?? [])
        .filter((port) => port.direction === "input" && samePortType(source, port.event_type))
        .map((port) => ({ nodeId: node.node_id, portId: port.port_id, label: `${desc?.title ?? node.node_type_id} · ${port.title}` }));
    });
}

export function removeNode(flow: FlowDocument, nodeId: string): void {
  flow.nodes = flow.nodes.filter((node) => node.node_id !== nodeId);
  flow.edges = flow.edges.filter((edge) => edge.from_node_id !== nodeId && edge.to_node_id !== nodeId);
}

export function removeEdge(flow: FlowDocument, edgeId: string): void {
  flow.edges = flow.edges.filter((edge) => edge.edge_id !== edgeId);
}

export function portTitle(flow: FlowDocument, catalog: NodeDescriptor[], nodeId: string, portId: string, direction: "input" | "output"): string {
  if (portId === "error") return "错误";
  const node = flow.nodes.find((item) => item.node_id === nodeId);
  return descriptorOf(catalog, node)?.ports.find((port) => port.port_id === portId && port.direction === direction)?.title ?? portId;
}

export interface MenuEntry {
  header?: boolean;
  id: string;
  label: string;
  keywords: string[];
  descriptor?: NodeDescriptor;
}

/** Catalog grouped by role, then category, as the add-node menu lists it. */
export function nodeMenu(catalog: NodeDescriptor[], roleLabel: (role: NodeRole) => string): MenuEntry[] {
  const entries: MenuEntry[] = [];
  for (const role of ROLE_ORDER) {
    const categories = new Map<string, NodeDescriptor[]>();
    for (const desc of catalog.filter((item) => item.role === role)) {
      const category = desc.category || "其他";
      categories.set(category, [...(categories.get(category) ?? []), desc]);
    }
    for (const [category, nodes] of categories) {
      entries.push({ header: true, id: `${role}:${category}`, label: `${roleLabel(role)} · ${category}`, keywords: [] });
      for (const desc of nodes) {
        entries.push({
          id: `${desc.node_type_id}@${desc.version}`,
          label: desc.title,
          keywords: [desc.node_type_id, category, roleLabel(role)],
          descriptor: desc,
        });
      }
    }
  }
  return entries;
}

/** Search keeps a header only when one of its entries matches. */
export function filterMenu(entries: MenuEntry[], query: string): MenuEntry[] {
  const needle = query.trim().toLocaleLowerCase();
  if (!needle) return entries;
  const out: MenuEntry[] = [];
  let header: MenuEntry | null = null;
  for (const entry of entries) {
    if (entry.header) {
      header = entry;
      continue;
    }
    if ([entry.label, ...entry.keywords].join(" ").toLocaleLowerCase().includes(needle)) {
      if (header) {
        out.push(header);
        header = null;
      }
      out.push(entry);
    }
  }
  return out;
}
