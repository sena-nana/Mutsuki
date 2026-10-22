<script setup lang="ts">
import { MkButton, MkCheckbox, MkField, MkNumberInput, MkSelect, MkSwitch, MkTextInput } from "@mutsuki/ui";
import { useMessages } from "@mutsuki/web-sdk";
import { computed, reactive } from "vue";

import type { ConfigProperty, FlowNode, NodeDescriptor } from "./contract";
import type { CanvasPort, Endpoint } from "./flow-model";
import { messages } from "./messages";

/**
 * The selected node's config, edited in place from the catalog's JSON schema, plus a keyboard
 * path to wiring: each output lists the compatible inputs it can connect to.
 */
const props = defineProps<{
  node: FlowNode;
  descriptor: NodeDescriptor | undefined;
  outputs: CanvasPort[];
  targets: (portId: string) => Array<Endpoint & { label: string }>;
}>();
const emit = defineEmits<{ config: [config: Record<string, unknown>]; connect: [fromPort: string, to: Endpoint]; delete: [] }>();
const { t } = useMessages(messages);
const pending = reactive<Record<string, string>>({});

const fields = computed(() => {
  const required = new Set(props.descriptor?.config_schema?.required ?? []);
  return Object.entries(props.descriptor?.config_schema?.properties ?? {}).map(([key, schema]) => ({
    key,
    schema,
    title: schema.title || key,
    required: required.has(key),
    kind: kindOf(schema),
  }));
});

function kindOf(schema: ConfigProperty) {
  if (schema.type === "boolean") return "bool";
  if (schema.type === "integer" || schema.type === "number") return "number";
  if (schema.type === "string") return Array.isArray(schema.enum) ? "select" : "text";
  if (schema.type === "array" && Array.isArray(schema.items?.enum)) return "multi";
  if (schema.type === "array" && schema.items?.type === "array") return "words";
  if (schema.type === "array" && schema.items?.type === "object") return "params";
  if (schema.type === "array") return "list";
  return "unsupported";
}

const options = (schema: ConfigProperty) => {
  const values = schema.enum ?? schema.items?.enum ?? [];
  const titles = schema.enumTitles ?? schema.items?.enumTitles ?? [];
  return values.map((value, index) => ({ value, label: titles[index] || value }));
};

const value = (key: string) => props.node.config[key];
const list = (key: string) => (Array.isArray(value(key)) ? (value(key) as unknown[]) : []);
const set = (key: string, next: unknown) => emit("config", { ...props.node.config, [key]: next });

function toggle(key: string, option: string, on: boolean) {
  const next = new Set(list(key).map(String));
  if (on) next.add(option);
  else next.delete(option);
  set(key, [...next]);
}

function add(key: string, kind: string) {
  const text = (pending[key] ?? "").trim();
  if (!text) return;
  const item = kind === "words" ? text.split(/\s+/) : kind === "params" ? { name: text, kind: "string", optional: false, variadic: false } : text;
  set(key, [...list(key), item]);
  pending[key] = "";
}

const itemLabel = (item: unknown, index: number) =>
  Array.isArray(item) ? item.join(" ") : item && typeof item === "object" ? String((item as { name?: string }).name ?? `#${index + 1}`) : String(item);

function connect(portId: string, choice: string | null) {
  const target = props.targets(portId).find((item) => `${item.nodeId}|${item.portId}` === choice);
  if (target) emit("connect", portId, target);
}
</script>

<template>
  <div class="flow-inspector">
    <p v-if="descriptor" class="lilia-node-editor__hint">{{ descriptor.title }}</p>
    <p v-if="!fields.length" class="muted">{{ t("inspector.noConfig") }}</p>
    <template v-for="field in fields" :key="field.key">
      <MkField v-if="field.kind === 'bool'" :label="field.title" :hint="field.schema.description" :required="field.required">
        <MkSwitch :model-value="!!value(field.key)" @update:model-value="set(field.key, $event)" />
      </MkField>
      <MkField v-else-if="field.kind === 'number'" :label="field.title" :hint="field.schema.description" :required="field.required">
        <MkNumberInput
          :model-value="typeof value(field.key) === 'number' ? (value(field.key) as number) : null"
          :step="field.schema.type === 'integer' ? 1 : undefined"
          @update:model-value="set(field.key, $event ?? undefined)"
        />
      </MkField>
      <MkField v-else-if="field.kind === 'select'" :label="field.title" :hint="field.schema.description" :required="field.required">
        <MkSelect
          :model-value="(value(field.key) as string | undefined) ?? (field.schema.default as string | undefined) ?? null"
          :options="options(field.schema)"
          @update:model-value="set(field.key, $event ?? undefined)"
        />
      </MkField>
      <MkField v-else-if="field.kind === 'text'" :label="field.title" :hint="field.schema.description" :required="field.required">
        <MkTextInput :model-value="String(value(field.key) ?? '')" @update:model-value="set(field.key, $event)" />
      </MkField>
      <fieldset v-else-if="field.kind !== 'unsupported'" class="flow-inspector__group">
        <legend>{{ field.title }}<span v-if="field.required" aria-hidden="true"> *</span></legend>
        <p v-if="field.schema.description" class="mk-field__hint">{{ field.schema.description }}</p>
        <template v-if="field.kind === 'multi'">
          <MkCheckbox
            v-for="option in options(field.schema)"
            :key="option.value"
            :model-value="list(field.key).includes(option.value)"
            @update:model-value="toggle(field.key, option.value, $event)"
            >{{ option.label }}</MkCheckbox
          >
        </template>
        <template v-else>
          <div v-for="(item, index) in list(field.key)" :key="index" class="flow-inspector__row">
            <span>{{ itemLabel(item, index) }}</span>
            <MkButton
              size="sm"
              variant="ghost"
              :aria-label="`${t('inspector.remove')} ${t('inspector.itemOf', { field: field.title, index: index + 1 })}`"
              @click="set(field.key, list(field.key).filter((_, i) => i !== index))"
            >
              {{ t("inspector.remove") }}
            </MkButton>
          </div>
          <form class="flow-inspector__row" @submit.prevent="add(field.key, field.kind)">
            <MkTextInput
              :model-value="pending[field.key] ?? ''"
              :label="field.kind === 'words' ? t('inspector.words') : field.kind === 'params' ? t('inspector.parameter') : field.title"
              :placeholder="field.kind === 'words' ? t('inspector.words') : field.kind === 'params' ? t('inspector.parameter') : undefined"
              @update:model-value="pending[field.key] = $event"
            />
            <MkButton size="sm" type="submit">{{ t("inspector.add") }}</MkButton>
          </form>
        </template>
      </fieldset>
    </template>
    <MkField v-for="port in outputs" :key="port.port_id" :label="t('inspector.connect', { port: port.title })">
      <MkSelect
        :model-value="null"
        :options="targets(port.port_id).map((item) => ({ value: `${item.nodeId}|${item.portId}`, label: item.label }))"
        :placeholder="targets(port.port_id).length ? t('inspector.connectPick') : t('inspector.connectNone')"
        :disabled="!targets(port.port_id).length"
        @update:model-value="connect(port.port_id, $event)"
      />
    </MkField>
    <div class="toolbar">
      <MkButton size="sm" variant="danger" @click="emit('delete')">{{ t("inspector.deleteNode") }}</MkButton>
    </div>
  </div>
</template>
