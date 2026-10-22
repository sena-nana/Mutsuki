import { defineRpc, mutation, query } from "@mutsuki/web-sdk";

export interface TypeRef {
  type_id: string;
  version: number;
}

export type NodeRole = "source" | "match" | "processor" | "sink";

export interface PortDescriptor {
  port_id: string;
  title: string;
  direction: "input" | "output";
  event_type: TypeRef;
  required?: boolean;
}

/** JSON-Schema subset the catalog uses for node config. */
export interface ConfigProperty {
  type?: "boolean" | "integer" | "number" | "string" | "array" | "object";
  title?: string;
  description?: string;
  default?: unknown;
  enum?: string[];
  enumTitles?: string[];
  items?: ConfigProperty;
}

export interface NodeDescriptor {
  node_type_id: string;
  version: number;
  title: string;
  category: string;
  role: NodeRole;
  ports: PortDescriptor[];
  config_schema: { properties?: Record<string, ConfigProperty>; required?: string[] };
}

export interface FlowNode {
  node_id: string;
  node_type_id: string;
  node_type_version: number;
  config: Record<string, unknown>;
  source?: { protocol_id: string; event_type: TypeRef | null } | null;
  position: { x: number; y: number };
}

export interface FlowEdge {
  edge_id: string;
  from_node_id: string;
  from_port_id: string;
  to_node_id: string;
  to_port_id: string;
  kind: "event" | "error";
}

export interface FlowDocument {
  flow_id: string;
  name: string;
  nodes: FlowNode[];
  edges: FlowEdge[];
}

export interface ValidationIssue {
  code: string;
  message: string;
  severity: "error" | "warning";
  node_id?: string | null;
  edge_id?: string | null;
}

export interface ValidationResult {
  valid: boolean;
  issues: ValidationIssue[];
}

/** `bot-flow-editor.*` RPCs; the namespace predates the `mutsuki.bot.flow` extension id. */
export const flowApi = defineRpc({
  namespace: "bot-flow-editor",
  methods: {
    "catalog.read": query<void, NodeDescriptor[] | { nodes: NodeDescriptor[] }>({ capability: "bot.flow.read" }),
    "snapshot.read": query<void, { revision: number; flow: FlowDocument | null }>({ capability: "bot.flow.read" }),
    validate: mutation<{ flow: FlowDocument }, ValidationResult>({ capability: "bot.flow.write" }),
    apply: mutation<{ expected_revision: number; flow: FlowDocument }, unknown>({ capability: "bot.flow.write" }),
  },
});
