<script setup lang="ts">
import { useI18n } from "@mutsuki/web-sdk";
import { nextTick, onMounted, ref, useId } from "vue";

/** Token login. The token goes to `/auth/session` once and is not kept anywhere. */
const props = defineProps<{ brand: string; error?: string; submit: (token: string) => Promise<void> }>();
const { t } = useI18n();
const token = ref("");
const pending = ref(false);
const input = ref<HTMLInputElement | null>(null);
const titleId = useId();
const errorId = useId();

async function onSubmit() {
  if (pending.value || !token.value) return;
  pending.value = true;
  try {
    await props.submit(token.value);
  } finally {
    token.value = "";
    pending.value = false;
    await nextTick();
    input.value?.focus();
  }
}

onMounted(() => input.value?.focus());
</script>

<template>
  <main class="console-login">
    <form
      class="console-login__card"
      :aria-labelledby="titleId"
      :aria-busy="pending ? 'true' : 'false'"
      @submit.prevent="onSubmit"
    >
      <div class="console-login__brand">
        <span class="console-login__brand-mark" aria-hidden="true" />
        {{ brand }}
      </div>
      <h1 :id="titleId">{{ t("shell.login.title") }}</h1>
      <p class="console-login__description">{{ t("shell.login.description") }}</p>
      <label class="console-login__field">
        <span>{{ t("shell.login.token") }}</span>
        <input
          ref="input"
          v-model="token"
          name="token"
          type="password"
          autocomplete="current-password"
          :aria-describedby="errorId"
          :aria-invalid="error ? 'true' : 'false'"
          :disabled="pending"
          required
        />
      </label>
      <p :id="errorId" class="console-login__error" role="alert">{{ error }}</p>
      <button class="console-login__submit" type="submit" :disabled="pending">
        {{ pending ? t("shell.login.pending") : t("shell.login.submit") }}
      </button>
    </form>
  </main>
</template>
