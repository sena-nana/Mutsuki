<script setup lang="ts">
import { MkButton, MkCard, MkField, MkTextInput, useConfirm, useToast } from "@mutsuki/ui";
import { useMessages, useRpc, useSession } from "@mutsuki/web-sdk";
import { computed, onBeforeUnmount, reactive, ref } from "vue";

import { bilibiliApi, bilibiliChanged, type LoginStatus } from "./contract";
import { messages } from "./messages";
import { useBilibiliAction } from "./useBilibiliAction";

/** Rendered by the platforms page under the Bilibili account (`platform.detail:bilibili`). */
defineProps<{ adapter?: unknown; account?: unknown }>();

const POLL_INTERVAL_MS = 2_000;

const { t } = useMessages(messages);
const session = useSession();
const confirm = useConfirm();
const toast = useToast();
const run = useBilibiliAction();
const status = useRpc(bilibiliApi.status, undefined, { refreshOn: [bilibiliChanged] });
const canWrite = computed(() => session.can("runtime.write"));

const qr = ref("");
const loginState = ref<LoginStatus | "idle">("idle");
const loginMessage = ref("");
const starting = ref(false);
let pollTimer: ReturnType<typeof setTimeout> | undefined;

function stopPolling() {
  clearTimeout(pollTimer);
  pollTimer = undefined;
}

async function poll() {
  pollTimer = undefined;
  const result = await run(bilibiliApi["login.poll"], undefined);
  if (!result) {
    loginState.value = "idle";
    return;
  }
  loginState.value = result.status;
  loginMessage.value = result.message;
  if (result.status === "confirmed") {
    qr.value = "";
    toast.success(t("login.confirmed"));
    await status.refresh();
  } else if (result.status === "expired") {
    qr.value = "";
  } else {
    pollTimer = setTimeout(() => void poll(), POLL_INTERVAL_MS);
  }
}

async function startLogin() {
  stopPolling();
  starting.value = true;
  try {
    const started = await run(bilibiliApi["login.start"], undefined);
    if (!started) return;
    qr.value = `data:image/png;base64,${started.qr_png_base64}`;
    loginState.value = "pending";
    loginMessage.value = "";
    pollTimer = setTimeout(() => void poll(), POLL_INTERVAL_MS);
  } finally {
    starting.value = false;
  }
}

const loginText = computed(() => {
  if (loginState.value === "idle") return t(status.data.value?.credential_loaded ? "login.loggedIn" : "login.idle");
  return t(`login.${loginState.value}`);
});

const binding = reactive({ operator: "", uid: "", group: "", result: "" });
const bindingBusy = ref(false);
const bindingReady = computed(() => binding.operator.trim() !== "" && Number(binding.uid) > 0);

async function bindingStep(step: () => Promise<void>) {
  bindingBusy.value = true;
  try {
    await step();
  } finally {
    bindingBusy.value = false;
  }
}

const bindStart = () =>
  bindingStep(async () => {
    const challenge = await run(bilibiliApi["binding.start"], {
      operator_user_id: binding.operator.trim(),
      uid: Number(binding.uid),
    });
    if (challenge) binding.result = t("binding.challenge", { code: challenge.code, name: challenge.name });
  });

const bindVerify = () =>
  bindingStep(async () => {
    const verified = await run(bilibiliApi["binding.verify"], {
      operator_user_id: binding.operator.trim(),
      platform: "web",
      target: { type: "group", group_id: binding.group.trim() },
    });
    if (!verified) return;
    if (verified.result === "signature_mismatch") {
      binding.result = t("binding.mismatch", { code: verified.code });
    } else {
      binding.result = "";
      toast.success(t("binding.verified"));
    }
  });

const unbind = () =>
  bindingStep(async () => {
    const operator = binding.operator.trim();
    const ok = await confirm({
      title: t("binding.unbind"),
      message: t("binding.unbindConfirm", { operator }),
      confirmLabel: t("binding.unbind"),
      danger: true,
    });
    if (!ok) return;
    const result = await run(bilibiliApi["binding.unbind"], { operator_user_id: operator });
    if (result) toast.info(t(result.removed ? "binding.unbound" : "binding.notBound"));
  });

onBeforeUnmount(stopPolling);
</script>

<template>
  <MkCard :title="t('login.title')">
    <div class="bilibili-login">
      <img v-if="qr" class="bilibili-login__qr" :src="qr" :alt="t('login.qrAlt')" width="192" height="192" />
      <p class="muted" role="status" aria-live="polite">{{ loginText }}</p>
    </div>
    <template #footer>
      <MkButton :disabled="!canWrite" :loading="starting" @click="startLogin">
        {{ loginState === "idle" ? t("login.start") : t("login.restart") }}
      </MkButton>
    </template>
  </MkCard>
  <MkCard v-if="status.data.value?.management_enabled && status.data.value.allow_self_binding" :title="t('binding.title')">
    <p class="muted">{{ t("binding.hint") }}</p>
    <form class="mk-overlay__form" @submit.prevent="bindStart">
      <MkField :label="t('binding.operator')" required><MkTextInput v-model="binding.operator" /></MkField>
      <MkField :label="t('binding.uid')"><MkTextInput v-model="binding.uid" /></MkField>
      <MkField :label="t('binding.group')"><MkTextInput v-model="binding.group" /></MkField>
    </form>
    <p v-if="binding.result" class="bilibili-binding__result" role="status">{{ binding.result }}</p>
    <template #footer>
      <MkButton :disabled="!canWrite || !bindingReady || bindingBusy" @click="bindStart">{{ t("binding.start") }}</MkButton>
      <MkButton :disabled="!canWrite || !bindingReady || !binding.group.trim() || bindingBusy" @click="bindVerify">
        {{ t("binding.verify") }}
      </MkButton>
      <MkButton variant="danger" :disabled="!canWrite || !binding.operator.trim() || bindingBusy" @click="unbind">
        {{ t("binding.unbind") }}
      </MkButton>
    </template>
  </MkCard>
</template>

<style>
.bilibili-login {
  display: flex;
  align-items: center;
  gap: var(--mk-space-4);
  flex-wrap: wrap;
}
.bilibili-login__qr {
  border-radius: var(--mk-radius-sm);
  background: var(--mk-color-surface);
}
.bilibili-binding__result {
  margin: var(--mk-space-3) 0 0;
}
</style>
