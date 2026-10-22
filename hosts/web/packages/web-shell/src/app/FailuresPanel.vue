<script setup lang="ts">
import { MkButton, MkDrawer, MkEmptyState, MkStatusBadge } from "@mutsuki/ui";
import { useI18n } from "@mutsuki/web-sdk";

import { useShell } from "./context.js";

const open = defineModel<boolean>("open", { required: true });
const { failures } = useShell();
const { t } = useI18n();
const reload = () => window.location.reload();
</script>

<template>
  <MkDrawer v-model:open="open" :title="t('shell.failures.title')">
    <MkEmptyState v-if="!failures.length" :title="t('shell.failures.empty')" icon="check" />
    <ul v-else class="shell-failures">
      <li v-for="(failure, index) in failures" :key="index" class="shell-failures__item">
        <div class="shell-failures__head">
          <strong>{{ failure.extensionId }}</strong>
          <MkStatusBadge tone="err" :label="t(`shell.failures.stage.${failure.stage ?? 'load'}`)" />
        </div>
        <p class="shell-failures__message">{{ failure.message }}</p>
      </li>
    </ul>
    <template #footer>
      <MkButton icon="refresh" @click="reload">{{ t("shell.failures.reload") }}</MkButton>
    </template>
  </MkDrawer>
</template>
