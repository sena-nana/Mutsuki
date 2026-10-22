import { useToast } from "@mutsuki/ui";
import { callRpc, toRpcError, useMessages, useMutsukiRuntime, useRpc, type JsonValue } from "@mutsuki/web-sdk";
import { computed, inject, provide, ref, shallowRef, watch, type InjectionKey } from "vue";

import { sandboxApi, sandboxChanged, type Conversation, type Message, type SandboxAction, type Segment, type Snapshot } from "./contract";
import { canQuote, resolveIdentity } from "./content";
import { messages as text } from "./messages";

/**
 * Page state shared by the session list, chat and member panes: the snapshot, the selected
 * conversation and its messages, the simulated identity and the quoted message. Every write
 * goes through `write`, which reports failures as a toast and resolves to `undefined`.
 */
export function createSandboxStore() {
  const { client } = useMutsukiRuntime();
  const toast = useToast();
  const { t } = useMessages(text);
  const snapshot = useRpc(sandboxApi.snapshot, { query: "" }, { refreshOn: [sandboxChanged] });
  const selectedId = ref<string | null>(null);
  const messages = shallowRef<Message[]>([]);
  const identityId = ref("");
  const quote = ref<Message | null>(null);
  /** Draft attachments (mentions, media, stickers, buttons); members can add mentions too. */
  const segments = ref<Segment[]>([]);

  const data = computed<Snapshot | null>(() => snapshot.data.value ?? null);
  const mode = computed(() => data.value?.mode ?? "simulate");
  const live = computed(() => mode.value === "live");
  const conversation = computed<Conversation | null>(
    () => data.value?.conversations.find((item) => item.conversation_id === selectedId.value) ?? null,
  );

  const fail = (error: unknown) => toast.error(t("failed", { message: toRpcError(error).message }));

  /** Drops a quote that no longer belongs to this conversation or this speaker. */
  function settleQuote() {
    const current = quote.value;
    if (!current) return;
    const valid =
      current.conversation_id === selectedId.value &&
      messages.value.some((item) => item.message_id === current.message_id) &&
      canQuote(current, identityId.value, live.value);
    if (!valid) quote.value = null;
  }

  async function loadMessages() {
    const id = selectedId.value;
    if (!id) {
      messages.value = [];
      quote.value = null;
      return;
    }
    try {
      const list = await callRpc(client, sandboxApi.messages, { conversation_id: id });
      if (selectedId.value !== id) return;
      messages.value = list ?? [];
      if (!live.value) identityId.value = resolveIdentity(conversation.value, data.value, identityId.value);
      settleQuote();
    } catch (error) {
      fail(error);
    }
  }

  // A new snapshot keeps the selection while it exists, else selects the first conversation.
  watch(data, (next) => {
    if (!next) return;
    if (!selectedId.value || !next.conversations.some((item) => item.conversation_id === selectedId.value)) {
      selectedId.value = next.conversations[0]?.conversation_id ?? null;
    }
    void loadMessages();
  });
  watch(selectedId, () => {
    quote.value = null;
    void loadMessages();
  });

  async function write(action: SandboxAction, confirmed = false): Promise<{ revision: number; result: JsonValue } | undefined> {
    try {
      const result = await callRpc(client, sandboxApi.write, {
        confirmed,
        request: { operation_id: crypto.randomUUID(), expected_revision: data.value?.revision ?? 0, action },
      });
      await snapshot.refresh();
      return result;
    } catch (error) {
      fail(error);
      return undefined;
    }
  }

  return {
    client,
    snapshot,
    data,
    mode,
    live,
    selectedId,
    conversation,
    messages,
    identityId,
    quote,
    segments,
    write,
    fail,
    settleQuote,
    loadMessages,
  };
}

export type SandboxStore = ReturnType<typeof createSandboxStore>;

const KEY: InjectionKey<SandboxStore> = Symbol("mutsuki.sandbox.store");

export function provideSandboxStore(store: SandboxStore) {
  provide(KEY, store);
}

export function useSandboxStore(): SandboxStore {
  const store = inject(KEY, null);
  if (!store) throw new Error("sandbox store is not provided");
  return store;
}
