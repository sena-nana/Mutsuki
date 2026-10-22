<script setup lang="ts">
import { MkButton, MkCheckbox, MkField, MkSegmented, MkTextarea, MkTextInput } from "@mutsuki/ui";
import { useMessages } from "@mutsuki/web-sdk";
import { computed } from "vue";

import { messages } from "../messages";
import type { FieldRendererProps } from "../renderers";

type Mode = "all" | "none" | "selected";

interface Persona {
  persona_id: string;
  system_prompt: string;
  custom_error_message: string;
  begin_dialogs: string[];
  tools_mode: Mode;
  allowed_tools: string[];
  skills_mode: Mode;
  allowed_skills: string[];
  available_tools?: string[];
  available_skills?: string[];
}

/**
 * `persona-card` format: the local Agent's persona object. An absent persona starts from the
 * legacy flat `assistant_instruction` so the first edit does not drop the existing prompt.
 */
const props = defineProps<FieldRendererProps>();
const emit = defineEmits<{ "update:modelValue": [value: Persona] }>();
const { t } = useMessages(messages);

const unique = (values: unknown) => [...new Set((Array.isArray(values) ? values : []).map((item) => String(item ?? "").trim()).filter(Boolean))];
const mode = (value: unknown): Mode => (value === "none" || value === "selected" ? value : "all");

const persona = computed<Persona>(() => {
  const source = (props.modelValue && typeof props.modelValue === "object" ? props.modelValue : {}) as Partial<Persona>;
  const legacy = typeof props.scope.assistant_instruction === "string" ? props.scope.assistant_instruction : "";
  const dialogs = (Array.isArray(source.begin_dialogs) ? source.begin_dialogs : []).map((item) => String(item ?? ""));
  if (dialogs.length % 2) dialogs.push("");
  return {
    persona_id: source.persona_id || "default",
    system_prompt: source.system_prompt ?? legacy,
    custom_error_message: source.custom_error_message ?? "",
    begin_dialogs: dialogs,
    tools_mode: mode(source.tools_mode),
    allowed_tools: unique(source.allowed_tools),
    skills_mode: mode(source.skills_mode),
    allowed_skills: unique(source.allowed_skills),
    available_tools: unique(source.available_tools),
    available_skills: unique(source.available_skills),
  };
});
const modes = computed(() => [
  { value: "all" as Mode, label: t("persona.modeAll") },
  { value: "none" as Mode, label: t("persona.modeNone") },
  { value: "selected" as Mode, label: t("persona.modeSelected") },
]);
const toolChoices = computed(() => unique([...(persona.value.available_tools ?? []), ...persona.value.allowed_tools]));
const skillChoices = computed(() => unique([...(persona.value.available_skills ?? []), ...persona.value.allowed_skills]));
const pairs = computed(() =>
  Array.from({ length: persona.value.begin_dialogs.length / 2 }, (_, index) => ({
    index,
    user: persona.value.begin_dialogs[index * 2],
    assistant: persona.value.begin_dialogs[index * 2 + 1],
  })),
);

const patch = (next: Partial<Persona>) => emit("update:modelValue", { ...persona.value, ...next });

function toggle(key: "allowed_tools" | "allowed_skills", id: string, on: boolean) {
  const next = new Set(persona.value[key]);
  if (on) next.add(id);
  else next.delete(id);
  patch({ [key]: [...next] });
}

function setDialog(index: number, value: string) {
  patch({ begin_dialogs: persona.value.begin_dialogs.map((item, i) => (i === index ? value : item)) });
}
</script>

<template>
  <div class="persona-card">
    <div class="persona-card__col">
      <MkField :label="t('persona.id')">
        <MkTextInput :model-value="persona.persona_id" :disabled="disabled" @update:model-value="patch({ persona_id: $event })" />
      </MkField>
      <MkField :label="t('persona.prompt')" :hint="t('persona.promptHint')">
        <MkTextarea :model-value="persona.system_prompt" :rows="6" :disabled="disabled" @update:model-value="patch({ system_prompt: $event })" />
      </MkField>
      <MkField :label="t('persona.errorMessage')" :hint="t('persona.errorMessageHint')">
        <MkTextarea
          :model-value="persona.custom_error_message"
          :rows="2"
          :disabled="disabled"
          @update:model-value="patch({ custom_error_message: $event })"
        />
      </MkField>
    </div>
    <div class="persona-card__col">
      <MkSegmented :label="t('persona.tools')" :options="modes" :model-value="persona.tools_mode" @update:model-value="patch({ tools_mode: $event })" />
      <fieldset v-if="persona.tools_mode === 'selected'" class="config-fieldset">
        <legend class="config-fieldset__legend">{{ t("persona.toolList") }}</legend>
        <p v-if="!toolChoices.length" class="muted">{{ t("persona.noTools") }}</p>
        <div class="config-choices">
          <MkCheckbox
            v-for="id in toolChoices"
            :key="id"
            :model-value="persona.allowed_tools.includes(id)"
            :disabled="disabled"
            @update:model-value="toggle('allowed_tools', id, $event)"
            >{{ id }}</MkCheckbox
          >
        </div>
      </fieldset>
      <MkSegmented :label="t('persona.skills')" :options="modes" :model-value="persona.skills_mode" @update:model-value="patch({ skills_mode: $event })" />
      <fieldset v-if="persona.skills_mode === 'selected'" class="config-fieldset">
        <legend class="config-fieldset__legend">{{ t("persona.skillList") }}</legend>
        <p v-if="!skillChoices.length" class="muted">{{ t("persona.noSkills") }}</p>
        <div class="config-choices">
          <MkCheckbox
            v-for="id in skillChoices"
            :key="id"
            :model-value="persona.allowed_skills.includes(id)"
            :disabled="disabled"
            @update:model-value="toggle('allowed_skills', id, $event)"
            >{{ id }}</MkCheckbox
          >
        </div>
      </fieldset>
      <p v-else class="muted">{{ t("persona.skillsHint") }}</p>
      <fieldset class="config-fieldset">
        <legend class="config-fieldset__legend">{{ t("persona.dialogs") }}</legend>
        <p class="mk-field__hint">{{ t("persona.dialogsHint") }}</p>
        <div v-for="pair in pairs" :key="pair.index" class="persona-card__dialog-pair">
          <MkTextarea
            :model-value="pair.user"
            :rows="2"
            :label="t('persona.user', { index: pair.index + 1 })"
            :disabled="disabled"
            @update:model-value="setDialog(pair.index * 2, $event)"
          />
          <MkTextarea
            :model-value="pair.assistant"
            :rows="2"
            :label="t('persona.assistant', { index: pair.index + 1 })"
            :disabled="disabled"
            @update:model-value="setDialog(pair.index * 2 + 1, $event)"
          />
          <MkButton
            size="sm"
            variant="ghost"
            :disabled="disabled"
            @click="patch({ begin_dialogs: persona.begin_dialogs.filter((_, i) => i !== pair.index * 2 && i !== pair.index * 2 + 1) })"
          >
            {{ t("persona.removePair") }}
          </MkButton>
        </div>
        <MkButton size="sm" icon="plus" :disabled="disabled" @click="patch({ begin_dialogs: [...persona.begin_dialogs, '', ''] })">
          {{ t("persona.addPair") }}
        </MkButton>
      </fieldset>
    </div>
  </div>
</template>
