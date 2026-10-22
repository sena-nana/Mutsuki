import type { Component } from "vue";

import type { ValueKind } from "../renderers";
import ArrayField from "./ArrayField.vue";
import BoolField from "./BoolField.vue";
import CronField from "./CronField.vue";
import EnumField from "./EnumField.vue";
import MapField from "./MapField.vue";
import NumberField from "./NumberField.vue";
import ObjectField from "./ObjectField.vue";
import PersonaCard from "./PersonaCard.vue";
import SecretField from "./SecretField.vue";
import TextField from "./TextField.vue";

/** Built-in renderer per value kind. `null` renders nothing. */
export const BUILTIN_KINDS: Partial<Record<ValueKind, Component>> = {
  bool: BoolField,
  integer: NumberField,
  float: NumberField,
  string: TextField,
  file_ref: TextField,
  directory_ref: TextField,
  enum: EnumField,
  secret: SecretField,
  array: ArrayField,
  map: MapField,
  object: ObjectField,
};

/** Built-in `presentation.format` renderers; extensions add more through `config.field:<format>`. */
export const BUILTIN_FORMATS: Record<string, Component> = {
  "persona-card": PersonaCard,
  "cron-expression": CronField,
};
