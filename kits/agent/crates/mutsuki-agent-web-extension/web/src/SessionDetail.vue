<script setup lang="ts">
import { MkButton, MkCard, MkErrorState, MkEventLedger, MkField, MkSkeleton, MkStatusBadge, MkTextInput, useConfirm, type MkLedgerRecord } from "@mutsuki/ui";
import { callRpc, toRpcError, useMessages, useMutsukiRuntime, useRpc, type RpcError } from "@mutsuki/web-sdk";
import { computed, reactive, ref, shallowRef, watch } from "vue";

import { mergeBySequence, projectAgentEvents, type AgentRecord } from "./agent-events";
import { ACTIVE_TURN, EVENT_PAGE_LIMIT, agentApi, versionOf, type AgentEventEnvelope, type PermissionRequest } from "./contract";
import { messages } from "./messages";
import { useAgentAction } from "./useAgentAction";

const props = defineProps<{ sessionId: string }>();
const emit = defineEmits<{ closed: [] }>();
const { t } = useMessages(messages);
const { client } = useMutsukiRuntime();
const confirm = useConfirm();
const act = useAgentAction();
const session = useRpc(agentApi["sessions.get"], () => ({ session_id: props.sessionId }));
const state = useRpc(agentApi["sessions.state"], () => ({ session_id: props.sessionId }));
const answers = reactive<Record<string, string>>({});

// Event window: the newest page first, older pages on demand.
const events = shallowRef<AgentEventEnvelope[]>([]);
const page = ref({ lost: 0, truncated: false, loadingOlder: false });
const eventsError = ref<RpcError | null>(null);
const projection = computed(() => projectAgentEvents(events.value));
const firstSequence = computed(() => (events.value.length ? events.value[0].sequence : 1));
const hasOlder = computed(() => firstSequence.value > 1 || page.value.truncated);

async function loadEvents(after: number): Promise<AgentEventEnvelope[]> {
  const result = await callRpc(client, agentApi["sessions.events"], {
    session_id: props.sessionId,
    after_sequence: after,
    limit: EVENT_PAGE_LIMIT,
  });
  page.value = { ...page.value, lost: Number(result.lost ?? 0), truncated: !!result.truncated };
  return result.events ?? [];
}

watch(
  () => session.data.value?.next_event_sequence,
  async (next) => {
    if (next == null) return;
    try {
      events.value = await loadEvents(Math.max(0, next - EVENT_PAGE_LIMIT));
      eventsError.value = null;
    } catch (error) {
      eventsError.value = toRpcError(error);
    }
  },
  { immediate: true },
);

async function older() {
  page.value = { ...page.value, loadingOlder: true };
  try {
    const first = firstSequence.value;
    const before = (await loadEvents(Math.max(0, first - EVENT_PAGE_LIMIT - 1))).filter((event) => event.sequence < first);
    events.value = mergeBySequence(events.value, before);
  } catch (error) {
    eventsError.value = toRpcError(error);
  } finally {
    page.value = { ...page.value, loadingOlder: false };
  }
}

/** The raw envelopes behind one record (at most eight). */
async function inspect(record: MkLedgerRecord) {
  const details = [];
  for (const sequence of ((record as AgentRecord).sequences ?? []).slice(0, 8)) {
    details.push(await callRpc(client, agentApi["sessions.event_get"], { session_id: props.sessionId, sequence }));
  }
  return details;
}

function refresh() {
  void session.refresh();
  void state.refresh();
}

async function decide(request: PermissionRequest, decision: "approved" | "rejected") {
  const method = decision === "approved" ? agentApi["sessions.approve"] : agentApi["sessions.reject"];
  if (decision === "approved" && !(await confirm({ title: t("approvals.approve"), message: t("approvals.approveConfirm", { tool: request.tool, summary: request.summary }) }))) {
    return;
  }
  await act(method, { decision: { session_id: request.session_id, turn_id: request.turn_id, action_id: request.action_id, version: request.version, decision } }, decision === "approved" ? t("approvals.approved") : t("approvals.rejected"));
  refresh();
}

async function answer(interactionId: string) {
  const item = state.data.value?.pending_interactions.find((entry) => entry.interaction_id === interactionId);
  if (!item) return;
  await act(agentApi["sessions.interact"], {
    resolution: { session_id: item.session_id, turn_id: item.turn_id, version: item.version, interaction_id: item.interaction_id, accepted: true, response: { answer: answers[interactionId] ?? "" } },
  });
  refresh();
}

const activeTurn = computed(() => state.data.value?.turns.find((turn) => ACTIVE_TURN.includes(turn.status)) ?? null);
const title = computed(() => session.data.value?.title || props.sessionId);

async function cancelTurn() {
  const current = state.data.value;
  if (!current || !activeTurn.value) return;
  if (!(await confirm({ title: t("sessions.cancelTurn"), message: t("sessions.cancelTurnConfirm"), danger: true }))) return;
  await act(agentApi["sessions.cancel_turn"], { session_id: props.sessionId, turn_id: activeTurn.value.turn_id, expected_version: versionOf(current) });
  refresh();
}

async function close() {
  const current = state.data.value;
  if (!current) return;
  if (!(await confirm({ title: t("sessions.close"), message: t("sessions.closeConfirm", { title: title.value }), danger: true }))) return;
  if (await act(agentApi["sessions.close"], { session_id: props.sessionId, expected_version: versionOf(current) })) emit("closed");
}

const role = (value: string) => (value === "user" || value === "assistant" ? t(`sessions.roles.${value}`) : value);
</script>

<template>
  <MkErrorState v-if="session.error.value && !session.data.value" :error="session.error.value" @retry="refresh" />
  <MkSkeleton v-else-if="!session.data.value || !state.data.value" />
  <div v-else class="agent-session">
    <MkCard :title="title">
      <template #actions>
        <MkStatusBadge :label="t(`sessions.status.${state.data.value.status}` as never)" tone="info" />
        <MkButton v-if="activeTurn" size="sm" variant="danger" @click="cancelTurn">{{ t("sessions.cancelTurn") }}</MkButton>
        <MkButton v-else size="sm" variant="ghost" @click="close">{{ t("sessions.close") }}</MkButton>
      </template>
      <p class="muted">{{ t("sessions.turns", { count: session.data.value.turn_count }) }}</p>
      <ol class="agent-transcript" :aria-label="t('sessions.transcript')">
        <li v-for="(message, index) in session.data.value.messages" :key="index" :class="['agent-message', `is-${message.role}`]">
          <strong>{{ role(message.role) }}</strong>
          <div>{{ message.content }}</div>
        </li>
        <li v-if="!session.data.value.messages.length" class="muted">{{ t("sessions.noMessages") }}</li>
      </ol>
    </MkCard>
    <MkCard v-if="state.data.value.pending_approvals.length" :title="t('approvals.inbox')">
      <div v-for="item in state.data.value.pending_approvals" :key="item.request.action_id" class="tree-item row-item">
        <div>
          <strong>{{ item.request.tool }}</strong>
          <div class="muted">{{ item.request.summary }}</div>
        </div>
        <div class="row-actions">
          <MkButton size="sm" variant="primary" @click="decide(item.request, 'approved')">{{ t("approvals.approve") }}</MkButton>
          <MkButton size="sm" @click="decide(item.request, 'rejected')">{{ t("approvals.reject") }}</MkButton>
        </div>
      </div>
    </MkCard>
    <MkCard v-if="state.data.value.pending_interactions.length" :title="t('interactions.title')">
      <form v-for="item in state.data.value.pending_interactions" :key="item.interaction_id" class="tree-item" @submit.prevent="answer(item.interaction_id)">
        <MkField :label="item.prompt">
          <MkTextInput v-model="answers[item.interaction_id]" :placeholder="t('interactions.answer')" />
        </MkField>
        <MkButton type="submit" size="sm">{{ t("interactions.submit") }}</MkButton>
      </form>
    </MkCard>
    <MkCard :title="t('sessions.timeline')">
      <p v-if="eventsError" class="err-text" role="alert">{{ eventsError.message }}</p>
      <MkEventLedger
        :records="projection.records"
        :groups="projection.groups"
        :has-older="hasOlder"
        :loading-older="page.loadingOlder"
        :lost="page.lost"
        :truncated="page.truncated"
        :inspect="inspect"
        @older="older"
      />
    </MkCard>
  </div>
</template>
