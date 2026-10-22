<script setup lang="ts">
import { MkButton, MkCard, MkKeyValue, MkSegmented, MkSelect, useConfirm } from "@mutsuki/ui";
import { SUPPORTED_LOCALES, WEB_PROTOCOL_VERSION, useI18n } from "@mutsuki/web-sdk";
import { computed, ref } from "vue";

import type { WebUiThemePreference } from "../runtime.js";
import { useShell } from "./context.js";

const { runtime, theme, brand, logout } = useShell();
const { t, locale } = useI18n();
const confirm = useConfirm();
const preference = ref<WebUiThemePreference>(theme.preference);
const themes = computed(() => [
  { value: "system" as const, label: t("shell.settings.themeSystem") },
  { value: "light" as const, label: t("shell.settings.themeLight") },
  { value: "dark" as const, label: t("shell.settings.themeDark") },
]);
const locales = SUPPORTED_LOCALES.map((code) => ({ value: code, label: code }));
const about = computed(() => [
  { label: brand, value: "" },
  { label: t("shell.settings.protocol"), value: WEB_PROTOCOL_VERSION, mono: true },
  { label: t("shell.settings.capabilities"), value: runtime.session?.capabilities.length ?? 0 },
  {
    label: t("shell.settings.safeMode"),
    value: runtime.session?.safe_mode ? t("shell.settings.yes") : t("shell.settings.no"),
  },
  { label: t("shell.settings.extensions"), value: runtime.state.extensions.join("、") },
]);

function setTheme(value: WebUiThemePreference) {
  preference.value = value;
  theme.setPreference(value);
}

async function onLogout() {
  const ok = await confirm({
    title: t("shell.settings.logout"),
    message: t("shell.settings.logoutConfirm"),
    confirmLabel: t("shell.settings.logout"),
  });
  if (ok) await logout();
}
</script>

<template>
  <div class="page-body settings-page">
    <MkCard :title="t('shell.settings.appearance')">
      <div class="settings-row">
        <div class="settings-row__label"><strong>{{ t("shell.settings.theme") }}</strong></div>
        <MkSegmented
          :label="t('shell.settings.theme')"
          :options="themes"
          :model-value="preference"
          @update:model-value="setTheme"
        />
      </div>
    </MkCard>
    <MkCard :title="t('shell.settings.language')">
      <MkSelect
        v-model="locale"
        :label="t('shell.settings.language')"
        :options="locales"
        :disabled="locales.length < 2"
      />
    </MkCard>
    <MkCard :title="t('shell.settings.session')">
      <MkButton variant="danger" @click="onLogout">{{ t("shell.settings.logout") }}</MkButton>
    </MkCard>
    <MkCard :title="t('shell.settings.about')">
      <MkKeyValue :items="about" />
    </MkCard>
  </div>
</template>
