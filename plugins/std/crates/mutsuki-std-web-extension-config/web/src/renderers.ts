import { inject, markRaw, provide, type Component, type InjectionKey } from "vue";

import type { ConfigNode, ConfigValueType, Draft, ValidationIssue } from "./schema";

/**
 * Props every field renderer receives. A renderer emits `update:modelValue` with the next plain
 * value; `scope` is the object the field lives in (siblings for cross-field defaults).
 */
export interface FieldRendererProps {
  node: ConfigNode;
  type: ConfigValueType;
  modelValue: unknown;
  disabled: boolean;
  scope: Draft;
  path: string[];
  issues: Map<string, ValidationIssue[]>;
  /** Accessible name when the control is not inside a labelled field (array items, map values). */
  label?: string;
}

export type ValueKind = ConfigValueType["kind"];

export interface FieldRenderers {
  /** Renderer for `presentation.format`, if one is registered. */
  format(name: string | null | undefined): Component | undefined;
  /** Built-in renderer for a value kind. */
  kind(kind: ValueKind): Component | undefined;
}

const KEY: InjectionKey<FieldRenderers> = Symbol("mutsuki.config.field-renderers");

/** Builds the registry: built-in kinds plus format renderers (built-in and contributed). */
export function createFieldRenderers(
  kinds: Partial<Record<ValueKind, Component>>,
  formats: Record<string, Component>,
): FieldRenderers {
  const byKind = new Map(Object.entries(kinds).map(([kind, component]) => [kind, markRaw(component)]));
  const byFormat = new Map(Object.entries(formats).map(([format, component]) => [format, markRaw(component)]));
  return {
    format: (name) => (name ? byFormat.get(name) : undefined),
    kind: (kind) => byKind.get(kind),
  };
}

export function provideFieldRenderers(renderers: FieldRenderers): void {
  provide(KEY, renderers);
}

export function useFieldRenderers(): FieldRenderers {
  const renderers = inject(KEY, null);
  if (!renderers) throw new Error("SchemaForm field renderers are not provided");
  return renderers;
}

/** Fields whose control is a group of inputs: rendered as a fieldset with a legend. */
export function isComposite(node: ConfigNode, type: ConfigValueType, renderers: FieldRenderers): boolean {
  return (
    !!renderers.format(node.presentation?.format) ||
    type.kind === "object" ||
    type.kind === "array" ||
    type.kind === "map" ||
    (type.kind === "enum" && !!type.multi)
  );
}
