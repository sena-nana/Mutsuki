<script setup lang="ts">
import { MkButton, MkCard, MkErrorState, MkKeyValue, MkSkeleton, useToast } from "@mutsuki/ui";
import {
  callRpc,
  toRpcError,
  useMessages,
  useMutsukiRuntime,
  useSession,
  useSubscription,
  type RpcError,
} from "@mutsuki/web-sdk";
import { computed, ref, shallowRef, watch } from "vue";

import { configApi, revisionChanged } from "./contract";
import { messages } from "./messages";
import type { FieldRenderers } from "./renderers";
import SchemaForm from "./SchemaForm.vue";
import {
  candidateOf,
  defaultContext,
  draftOf,
  localized,
  providerIdOf,
  restartPolicies,
  secretFields,
  valueAt,
  type ConfigDescriptor,
  type ConfigSnapshot,
  type Draft,
  type SecretState,
  type ValidationResult,
} from "./schema";

/**
 * Edits one provider's document: validate, then apply against the revision the draft was read
 * at. A revision conflict keeps the draft and asks before discarding it; every failure is shown.
 */
const props = defineProps<{ providerId: string; renderers: FieldRenderers }>();
const { t } = useMessages(messages);
const { client } = useMutsukiRuntime();
const session = useSession();
const toast = useToast();

const schema = shallowRef<ConfigDescriptor | null>(null);
const snapshot = shallowRef<ConfigSnapshot | null>(null);
const baseline = ref<Draft>({});
const draft = ref<Draft>({});
const loadError = ref<RpcError | null>(null);
const actionError = ref<RpcError | null>(null);
const validation = ref<ValidationResult | null>(null);
const conflict = ref<{ current?: number; remote: boolean } | null>(null);
const busy = ref<"validate" | "apply" | null>(null);

const dirty = computed(() => JSON.stringify(draft.value) !== JSON.stringify(baseline.value));
const canApply = computed(() => session.can("config.apply"));
const failedIssues = computed(() => validation.value?.issues.filter((issue) => issue.severity === "error") ?? []);

async function load() {
  loadError.value = null;
  try {
    const descriptor = await callRpc(client, configApi["schema.get"], { provider_id: props.providerId });
    const read = await callRpc(client, configApi["snapshot.read"], {
      provider_id: props.providerId,
      context: defaultContext(descriptor),
    });
    schema.value = descriptor;
    snapshot.value = read;
    baseline.value = draftOf(read);
    draft.value = draftOf(read);
    validation.value = null;
    conflict.value = null;
    actionError.value = null;
  } catch (error) {
    loadError.value = toRpcError(error);
  }
}

watch(() => props.providerId, load, { immediate: true });

useSubscription(revisionChanged, (payload) => {
  if (!snapshot.value || busy.value === "apply") return;
  if (providerIdOf(payload.provider_id) !== props.providerId) return;
  const raw = payload.revision;
  const revision = typeof raw === "object" ? raw?.value : raw;
  if (revision === undefined || Number(revision) === Number(snapshot.value.revision)) return;
  // Nothing to lose: follow the new revision. With local edits, ask first.
  if (!dirty.value) void load();
  else conflict.value = { current: Number(revision), remote: true };
});

function request() {
  return { provider_id: props.providerId, context: defaultContext(schema.value!) };
}

async function runValidate(): Promise<boolean> {
  actionError.value = null;
  try {
    validation.value = await callRpc(client, configApi.validate, {
      ...request(),
      candidate: candidateOf(schema.value!, draft.value),
    });
    return validation.value.ok;
  } catch (error) {
    actionError.value = toRpcError(error);
    return false;
  }
}

async function validate() {
  busy.value = "validate";
  try {
    if (await runValidate()) toast.success(t("validOk"));
  } finally {
    busy.value = null;
  }
}

async function apply() {
  busy.value = "apply";
  try {
    if (!(await runValidate())) return;
    const policies = restartPolicies(schema.value!, baseline.value, draft.value);
    const result = await callRpc(client, configApi.apply, {
      ...request(),
      request: { candidate: candidateOf(schema.value!, draft.value), expected_revision: snapshot.value?.revision ?? 0, dry_run: false },
    });
    const restart =
      result.pending_actions.some((action) => action === "application_restart_scheduled" || action === "host_restart_scheduled") ||
      policies.some((policy) => policy === "application_restart" || policy === "host_restart");
    toast.success(t(restart ? "appliedRestart" : "applied"));
    await load();
  } catch (error) {
    const failure = toRpcError(error);
    if (failure.code === "config.revision_conflict") conflict.value = { remote: false };
    else actionError.value = failure;
  } finally {
    busy.value = null;
  }
}

function discard() {
  draft.value = structuredClone(baseline.value);
  validation.value = null;
  actionError.value = null;
}

const secrets = computed(() =>
  schema.value
    ? secretFields(schema.value.root.children ?? []).map(({ path, node }) => {
        const pending = (valueAt(draft.value, path) as SecretState | undefined)?.state;
        const stored = (valueAt(baseline.value, path) as SecretState | undefined)?.state;
        const state = pending === "set" || pending === "clear" ? "pending" : stored === "configured" ? "configured" : stored === "unavailable" ? "unavailable" : "absent";
        return { label: localized(node.title) || path.join("."), value: t(`secrets.${state}`) };
      })
    : [],
);
</script>

<template>
  <div class="config-editor">
    <MkErrorState v-if="loadError" :error="loadError" :title="t('loadFailed')" @retry="load" />
    <MkSkeleton v-else-if="!schema" :lines="6" />
    <template v-else>
      <div v-if="conflict" class="config-editor__banner" role="alert">
        <strong>{{ t("conflict.title") }}</strong>
        <p>
          {{
            conflict.current !== undefined
              ? t("conflict.body", { current: conflict.current, expected: snapshot?.revision ?? 0 })
              : t("conflict.remote")
          }}
        </p>
        <MkButton size="sm" @click="load">{{ t("conflict.reload") }}</MkButton>
      </div>
      <p v-if="actionError" class="error-banner" role="alert">
        {{ t(busy === "validate" ? "validateFailed" : "applyFailed", { message: actionError.message, code: actionError.code }) }}
      </p>
      <p v-if="failedIssues.length" class="error-banner" role="alert">{{ t("issues", { count: failedIssues.length }) }}</p>
      <p v-if="localized(schema.description)" class="muted">{{ localized(schema.description) }}</p>
      <MkCard v-if="secrets.length" :title="t('secrets.title')" variant="flat">
        <MkKeyValue :items="secrets" />
      </MkCard>
      <SchemaForm v-model="draft" :schema="schema" :renderers="renderers" :validation="validation" :disabled="!canApply" />
      <div class="config-editor__actions toolbar row-item">
        <span v-if="dirty" class="muted" role="status">{{ t("unsaved") }}</span>
        <MkButton :loading="busy === 'validate'" :disabled="busy !== null" @click="validate">{{ t("validate") }}</MkButton>
        <MkButton variant="ghost" :disabled="!dirty || busy !== null" @click="discard">{{ t("discard") }}</MkButton>
        <MkButton
          variant="primary"
          :loading="busy === 'apply'"
          :disabled="!canApply || !dirty || busy !== null || (!!conflict && !conflict.remote)"
          @click="apply"
        >
          {{ t("apply") }}
        </MkButton>
      </div>
    </template>
  </div>
</template>

<style>
.config-editor {
  display: grid;
  gap: var(--mk-space-4);
}
.config-editor__banner {
  display: grid;
  gap: var(--mk-space-2);
  justify-items: start;
  padding: var(--mk-space-3);
  border: 1px solid var(--mk-color-warn);
  border-radius: var(--mk-radius-sm);
  background: var(--mk-color-warn-soft);
}
.config-editor__banner p {
  margin: 0;
}
.config-editor__actions {
  position: sticky;
  bottom: 0;
  padding: var(--mk-space-3) 0;
  background: var(--mk-color-surface);
  border-top: 1px solid var(--mk-color-border);
}
.config-form,
.config-form__fields,
.config-object,
.config-list {
  display: grid;
  gap: var(--mk-space-3);
}
.config-fieldset {
  display: grid;
  gap: var(--mk-space-2);
  margin: 0;
  padding: var(--mk-space-3);
  border: 1px solid var(--mk-color-border);
  border-radius: var(--mk-radius-sm);
  min-width: 0;
}
.config-fieldset__legend {
  padding: 0 var(--mk-space-1);
  font-weight: 600;
}
.config-list__row {
  display: flex;
  gap: var(--mk-space-2);
  align-items: flex-start;
}
.config-list__row > :first-child {
  flex: 1;
  min-width: 0;
}
.config-choices {
  display: flex;
  flex-wrap: wrap;
  gap: var(--mk-space-3);
}
.persona-card {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(18rem, 1fr));
  gap: var(--mk-space-4);
}
.persona-card__col {
  display: grid;
  gap: var(--mk-space-3);
  align-content: start;
}
.persona-card__dialog-pair {
  display: grid;
  grid-template-columns: 1fr 1fr auto;
  gap: var(--mk-space-2);
  align-items: start;
}
</style>
