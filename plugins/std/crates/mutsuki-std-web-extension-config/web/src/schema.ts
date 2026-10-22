/**
 * Mirror of `mutsuki-protocol-config`'s descriptor and value wire, plus the pure logic the form
 * needs: plain/wire conversion, visibility expressions, grouping and validation issue lookup.
 */

export interface LocalizedText {
  default: string;
  zh_cn?: string | null;
  en?: string | null;
}

export interface EnumOption {
  value: string;
  label: LocalizedText;
}

export type ConfigValueType =
  | { kind: "null" }
  | { kind: "bool" }
  | { kind: "integer" }
  | { kind: "float" }
  | { kind: "string"; multiline?: boolean }
  | { kind: "enum"; options: EnumOption[]; multi?: boolean }
  | { kind: "object" }
  | { kind: "array"; item: ConfigValueType }
  | { kind: "map"; key_strategy: unknown; value: ConfigValueType }
  | { kind: "secret" }
  | { kind: "file_ref" }
  | { kind: "directory_ref" };

export type ConfigExpr =
  | { op: "field"; key: string }
  | { op: "literal"; value: WireValue }
  | { op: "eq" | "ne"; left: ConfigExpr; right: ConfigExpr }
  | { op: "and" | "or"; items: ConfigExpr[] }
  | { op: "not"; expr: ConfigExpr }
  | { op: "is_set"; key: string };

export type RestartPolicy = "none" | "reconfigure" | "plugin_reload" | "application_restart" | "host_restart";

export interface ConfigNode {
  key: string;
  value_type: ConfigValueType;
  title: LocalizedText;
  description?: LocalizedText | null;
  default_value?: WireValue | null;
  constraints?: { required?: boolean; min?: number; max?: number };
  presentation?: {
    group?: string | null;
    order?: number;
    unit?: string | null;
    placeholder?: string | null;
    format?: string | null;
    secret?: boolean;
  };
  visibility?: ConfigExpr | null;
  enabled_if?: ConfigExpr | null;
  mutability?: "read_write" | "read_only" | "computed";
  restart_policy?: RestartPolicy;
  children?: ConfigNode[];
}

export interface ConfigDescriptor {
  provider_id: string;
  title: LocalizedText;
  description?: LocalizedText | null;
  scopes: string[];
  root: ConfigNode;
  groups?: Array<{ id: string; title: LocalizedText; order?: number }>;
}

export type SecretState =
  | { state: "configured" | "absent" | "unavailable" | "keep" | "clear" }
  | { state: "set"; value: string };

export type WireValue =
  | { type: "null" }
  | { type: "bool"; value: boolean }
  | { type: "integer"; value: number }
  | { type: "float"; value: number }
  | { type: "string"; value: string }
  | { type: "secret"; value: SecretState }
  | { type: "array"; value: WireValue[] }
  | { type: "object"; value: Record<string, WireValue> };

export interface ConfigContext {
  scope: string;
  qualifiers: Record<string, string>;
}

export interface ConfigSnapshot {
  value: WireValue;
  revision: number;
  source?: string;
}

export interface ValidationIssue {
  path: string[];
  code: string;
  severity: "error" | "warning";
  message: LocalizedText;
}

export interface ValidationResult {
  ok: boolean;
  issues: ValidationIssue[];
}

export type ConfigAction =
  | "persisted"
  | "reconfigured"
  | "plugin_reloaded"
  | "application_restart_scheduled"
  | "host_restart_scheduled"
  | "none";

export interface ApplyResult {
  revision: number;
  applied: boolean;
  actions: ConfigAction[];
  pending_actions: ConfigAction[];
}

/** Plain JSON draft value; secrets stay `SecretState` objects. */
export type Draft = Record<string, unknown>;

export const localized = (value: LocalizedText | null | undefined): string => value?.zh_cn || value?.default || "";

export function nodeTitle(node: ConfigNode): string {
  const title = localized(node.title) || node.key;
  return node.presentation?.unit ? `${title}（${node.presentation.unit}）` : title;
}

export const isSecret = (node: ConfigNode) => node.value_type.kind === "secret" || !!node.presentation?.secret;

export function fromWire(value: WireValue | null | undefined): unknown {
  if (!value) return undefined;
  switch (value.type) {
    case "null":
      return null;
    case "array":
      return value.value.map(fromWire);
    case "object":
      return Object.fromEntries(Object.entries(value.value).map(([key, child]) => [key, fromWire(child)]));
    default:
      return value.value;
  }
}

/** Snapshot root → editable draft. */
export function draftOf(snapshot: ConfigSnapshot | null | undefined): Draft {
  const plain = fromWire(snapshot?.value);
  return plain && typeof plain === "object" && !Array.isArray(plain) ? structuredClone(plain as Draft) : {};
}

/** Untyped JSON (objects without declared children) → wire. */
export function jsonToWire(value: unknown): WireValue {
  if (value === null || value === undefined) return { type: "null" };
  if (typeof value === "boolean") return { type: "bool", value };
  if (typeof value === "number") return Number.isInteger(value) ? { type: "integer", value } : { type: "float", value };
  if (typeof value === "string") return { type: "string", value };
  if (Array.isArray(value)) return { type: "array", value: value.map(jsonToWire) };
  return {
    type: "object",
    value: Object.fromEntries(Object.entries(value as Record<string, unknown>).map(([key, child]) => [key, jsonToWire(child)])),
  };
}

/** A read secret (`configured`, `absent`…) is never written back: only `set`/`clear` change it. */
function secretWrite(plain: unknown): SecretState {
  const state = (plain as SecretState | undefined)?.state;
  if (state === "set" || state === "clear") return plain as SecretState;
  return { state: "keep" };
}

/**
 * Plain → wire for one node. `undefined` means "absent": the key is omitted so optional fields
 * and sections stay unset instead of being written as empty values.
 */
export function toWire(node: ConfigNode, type: ConfigValueType, plain: unknown): WireValue | undefined {
  if (node.presentation?.secret || type.kind === "secret") return { type: "secret", value: secretWrite(plain) };
  if (plain === undefined) return undefined;
  switch (type.kind) {
    case "null":
      return { type: "null" };
    case "bool":
      return { type: "bool", value: !!plain };
    case "integer":
      return plain === null || plain === "" ? undefined : { type: "integer", value: Math.trunc(Number(plain)) };
    case "float":
      return plain === null || plain === "" ? undefined : { type: "float", value: Number(plain) };
    case "enum":
      if (type.multi) {
        return { type: "array", value: (Array.isArray(plain) ? plain : []).map((item) => ({ type: "string", value: String(item) })) };
      }
      return plain === null || plain === "" ? undefined : { type: "string", value: String(plain) };
    case "array": {
      const item = itemNode(node, type.item, "item");
      const items = (Array.isArray(plain) ? plain : []).map((value) => toWire(item, type.item, value) ?? { type: "null" as const });
      return { type: "array", value: items };
    }
    case "map": {
      const entry = itemNode(node, type.value, "value");
      const source = plain && typeof plain === "object" ? (plain as Record<string, unknown>) : {};
      const value: Record<string, WireValue> = {};
      for (const [key, child] of Object.entries(source)) value[key] = toWire(entry, type.value, child) ?? { type: "null" };
      return { type: "object", value };
    }
    case "object": {
      if (!node.children?.length) return jsonToWire(plain);
      const source = plain && typeof plain === "object" ? (plain as Record<string, unknown>) : {};
      return { type: "object", value: childrenToWire(node.children, source) };
    }
    default:
      return { type: "string", value: String(plain ?? "") };
  }
}

function childrenToWire(children: ConfigNode[], source: Record<string, unknown>): Record<string, WireValue> {
  const out: Record<string, WireValue> = {};
  const known = new Set<string>();
  for (const child of children) {
    known.add(child.key);
    const wire = toWire(child, child.value_type, source[child.key]);
    if (wire) out[child.key] = wire;
  }
  return out;
}

/** The whole draft as an apply/validate candidate. Hidden fields are kept, never dropped. */
export function candidateOf(schema: ConfigDescriptor, draft: Draft): WireValue {
  return { type: "object", value: childrenToWire(schema.root.children ?? [], draft) };
}

/** Synthetic node for array items and map values, inheriting editability from the owner. */
export function itemNode(owner: ConfigNode, type: ConfigValueType, key: string): ConfigNode {
  return { key, value_type: type, title: { default: key }, mutability: owner.mutability, presentation: {}, children: [] };
}

export function emptyValue(node: ConfigNode, type: ConfigValueType = node.value_type): unknown {
  const fallback = fromWire(node.default_value ?? undefined);
  if (fallback !== undefined) return structuredClone(fallback);
  switch (type.kind) {
    case "bool":
      return false;
    case "array":
      return [];
    case "map":
      return {};
    case "object":
      return Object.fromEntries((node.children ?? []).flatMap((child) => {
        const value = emptyValue(child);
        return value === undefined ? [] : [[child.key, value]];
      }));
    case "enum":
      return type.multi ? [] : undefined;
    case "string":
    case "file_ref":
    case "directory_ref":
      return "";
    default:
      return undefined;
  }
}

function atom(expr: ConfigExpr, scope: Draft): unknown {
  if (expr.op === "field") return scope[expr.key];
  if (expr.op === "literal") return fromWire(expr.value);
  return evaluate(expr, scope);
}

/** The restricted expression AST; unknown operators fail open (shown, editable). */
export function evaluate(expr: ConfigExpr, scope: Draft): boolean {
  switch (expr.op) {
    case "field":
    case "literal":
      return !!atom(expr, scope);
    case "eq":
      return JSON.stringify(atom(expr.left, scope)) === JSON.stringify(atom(expr.right, scope));
    case "ne":
      return JSON.stringify(atom(expr.left, scope)) !== JSON.stringify(atom(expr.right, scope));
    case "and":
      return expr.items.every((item) => evaluate(item, scope));
    case "or":
      return expr.items.some((item) => evaluate(item, scope));
    case "not":
      return !evaluate(expr.expr, scope);
    case "is_set":
      return scope[expr.key] !== undefined && scope[expr.key] !== null;
    default:
      return true;
  }
}

export const isVisible = (node: ConfigNode, scope: Draft) => !node.visibility || evaluate(node.visibility, scope);

export function isEditable(node: ConfigNode, scope: Draft): boolean {
  if (node.mutability === "read_only" || node.mutability === "computed") return false;
  return !node.enabled_if || evaluate(node.enabled_if, scope);
}

export function sortNodes(nodes: ConfigNode[]): ConfigNode[] {
  return nodes
    .map((node, index) => ({ node, index }))
    .sort((a, b) => (a.node.presentation?.order ?? 0) - (b.node.presentation?.order ?? 0) || a.index - b.index)
    .map((item) => item.node);
}

export interface FormGroup {
  id: string;
  title: string;
  nodes: ConfigNode[];
}

/** Declared groups in order, then groups only nodes name, then ungrouped fields. */
export function groupNodes(schema: ConfigDescriptor): FormGroup[] {
  const declared = [...(schema.groups ?? [])].sort((a, b) => (a.order ?? 0) - (b.order ?? 0));
  const buckets = new Map<string, ConfigNode[]>();
  const ungrouped: ConfigNode[] = [];
  for (const node of sortNodes(schema.root.children ?? [])) {
    const group = node.presentation?.group;
    if (!group) ungrouped.push(node);
    else buckets.set(group, [...(buckets.get(group) ?? []), node]);
  }
  const groups: FormGroup[] = declared.map((group) => ({
    id: group.id,
    title: localized(group.title) || group.id,
    nodes: buckets.get(group.id) ?? [],
  }));
  for (const group of declared) buckets.delete(group.id);
  for (const [id, nodes] of buckets) groups.push({ id, title: id, nodes });
  if (ungrouped.length) groups.push({ id: "", title: "", nodes: ungrouped });
  return groups.filter((group) => group.nodes.length);
}

/** Every secret field in the tree, with its dotted path. */
export function secretFields(nodes: ConfigNode[], prefix: string[] = []): Array<{ path: string[]; node: ConfigNode }> {
  return nodes.flatMap((node) => {
    const path = [...prefix, node.key];
    if (isSecret(node)) return [{ path, node }];
    return node.value_type.kind === "object" ? secretFields(node.children ?? [], path) : [];
  });
}

export function valueAt(draft: Draft, path: string[]): unknown {
  return path.reduce<unknown>((value, key) => (value && typeof value === "object" ? (value as Draft)[key] : undefined), draft);
}

export const pathKey = (path: readonly string[]) => path.join(".");

/** Issues keyed by dotted path. */
export function issuesByPath(result: ValidationResult | null | undefined): Map<string, ValidationIssue[]> {
  const map = new Map<string, ValidationIssue[]>();
  for (const issue of result?.issues ?? []) {
    const key = pathKey(issue.path ?? []);
    map.set(key, [...(map.get(key) ?? []), issue]);
  }
  return map;
}

/** First scope the provider declares; plugin-instance scopes target the default instance. */
export function defaultContext(schema: ConfigDescriptor): ConfigContext {
  const scope = schema.scopes?.[0] ?? "mutsuki.global";
  return scope === "mutsuki.global" ? { scope, qualifiers: {} } : { scope, qualifiers: { plugin_instance_id: "default" } };
}

/** Restart consequences of the fields that changed between two drafts. */
export function restartPolicies(schema: ConfigDescriptor, before: Draft, after: Draft): RestartPolicy[] {
  const policies = new Set<RestartPolicy>();
  for (const node of schema.root.children ?? []) {
    const policy = node.restart_policy ?? "none";
    if (policy !== "none" && JSON.stringify(before[node.key]) !== JSON.stringify(after[node.key])) policies.add(policy);
  }
  return [...policies];
}

/** Provider ids arrive as plain strings or `{ value }` newtypes. */
export const providerIdOf = (value: unknown): string =>
  typeof value === "string" ? value : String((value as { value?: unknown } | null)?.value ?? "");
