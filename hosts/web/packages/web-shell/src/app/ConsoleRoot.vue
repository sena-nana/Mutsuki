<script setup lang="ts">
import { MkSkeleton } from "@mutsuki/ui";
import { RpcError, useI18n } from "@mutsuki/web-sdk";
import { onBeforeUnmount, ref, shallowRef } from "vue";

import type { WebShellRuntime } from "../runtime.js";
import LoginView from "./LoginView.vue";

/**
 * Login → connect → load extensions → shell. `start` connects with the session cookie;
 * `unauthorized` (no or expired cookie) shows the login, any other failure a retryable error.
 */
const props = defineProps<{
  brand: string;
  start: () => Promise<WebShellRuntime>;
  login: (token: string) => Promise<boolean>;
}>();
const emit = defineEmits<{ ready: [runtime: WebShellRuntime] }>();
const { t } = useI18n();
const phase = ref<"connecting" | "login" | "ready">("connecting");
const loginError = ref("");
const runtime = shallowRef<WebShellRuntime | null>(null);
let stateWatch: { dispose(): void } | undefined;

async function connect(): Promise<void> {
  phase.value = "connecting";
  try {
    const started = await props.start();
    runtime.value = started;
    phase.value = "ready";
    stateWatch = started.bridge.onStateChange((state) => {
      // An expired session on reconnect: the next login rebuilds the whole console.
      if (state === "unauthorized") window.location.reload();
    });
    emit("ready", started);
  } catch (error) {
    const rejected = error instanceof RpcError && error.code === "auth_failed";
    loginError.value = rejected ? "" : t("shell.login.failed");
    phase.value = "login";
  }
}

async function submit(token: string): Promise<void> {
  loginError.value = "";
  let accepted = false;
  try {
    accepted = await props.login(token);
  } catch {
    loginError.value = t("shell.login.failed");
    return;
  }
  if (!accepted) {
    loginError.value = t("shell.login.rejected");
    return;
  }
  await connect();
}

onBeforeUnmount(() => stateWatch?.dispose());
void connect();
</script>

<template>
  <slot v-if="phase === 'ready' && runtime" :runtime="runtime" />
  <LoginView v-else-if="phase === 'login'" :brand="brand" :error="loginError" :submit="submit" />
  <main v-else class="console-login" :aria-busy="'true'">
    <span class="mk-visually-hidden" role="status">{{ t("shell.login.connecting") }}</span>
    <MkSkeleton variant="block" />
  </main>
</template>
