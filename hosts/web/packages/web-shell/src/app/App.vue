<script setup lang="ts">
import { MkEmptyState, MkLiveRegion, MkOverlayHost, MkSkipLink } from "@mutsuki/ui";
import { useConnectionState, useI18n } from "@mutsuki/web-sdk";
import { computed, ref, watch } from "vue";
import { useRoute } from "vue-router";

import { useActiveNavigation } from "../navigation.js";
import ActivityBar from "./ActivityBar.vue";
import ContextNav from "./ContextNav.vue";
import FailuresPanel from "./FailuresPanel.vue";
import PageView from "./PageView.vue";
import { useShell } from "./context.js";

const { model, brand, failures, runtime } = useShell();
const { t } = useI18n();
const route = useRoute();
const connection = useConnectionState();
const contextOpen = ref(false);
const failuresOpen = ref(false);

const current = computed(() => {
  const activityId = route.params.activity;
  const pageId = route.params.page;
  return typeof activityId === "string" && typeof pageId === "string"
    ? { activityId, pageId }
    : null;
});
const { activity, items, page } = useActiveNavigation(model, current);
const showContext = computed(() => items.value.length > 1);

const connectionLabel = computed(() => {
  const label = t(`shell.connection.${connection.value}`);
  return runtime.session?.safe_mode ? t("shell.connection.safeMode", { state: label }) : label;
});
const connectionQuiet = computed(() => connection.value === "open" && !runtime.session?.safe_mode);

watch(
  page,
  (next) => {
    contextOpen.value = false;
    document.title = next ? `${next.title} | ${brand}` : brand;
  },
  { immediate: true },
);
</script>

<template>
  <MkSkipLink target="#main" />
  <div
    :class="[
      'mutsuki-console',
      'mutsuki-console--activity-shell',
      { 'is-context-hidden': !showContext, 'is-context-open': contextOpen },
    ]"
  >
    <ActivityBar :active-id="current?.activityId" />
    <ContextNav
      v-if="showContext && current"
      :activity-id="current.activityId"
      :title="activity?.label ?? ''"
      :page-id="current.pageId"
      @navigate="contextOpen = false"
    />
    <button
      class="console-context-mask"
      type="button"
      :aria-label="t('shell.closeNavigation')"
      @click="contextOpen = false"
    />
    <main id="main" class="console-main" tabindex="-1">
      <header class="console-page-header">
        <button
          class="console-menu-toggle"
          type="button"
          :aria-label="t('shell.openNavigation')"
          :aria-expanded="contextOpen ? 'true' : 'false'"
          @click="contextOpen = !contextOpen"
        >
          ☰
        </button>
        <h1>{{ page?.title ?? brand }}</h1>
        <div id="mk-page-actions" class="console-page-header__actions">
          <button
            v-if="failures.length"
            type="button"
            class="console-extension-retry"
            @click="failuresOpen = true"
          >
            {{ t("shell.failures.button", { count: failures.length }) }}
          </button>
          <span
            class="console-connection-dot"
            :data-state="connection"
            :hidden="connectionQuiet"
            aria-hidden="true"
          />
          <span class="console-connection-label" :hidden="connectionQuiet">{{ connectionLabel }}</span>
        </div>
      </header>
      <section class="console-page-content">
        <PageView v-if="page" :key="page.id" :page="page" />
        <MkEmptyState v-else :title="t('shell.noPages')" />
      </section>
    </main>
  </div>
  <FailuresPanel v-model:open="failuresOpen" />
  <MkLiveRegion :message="connectionLabel" />
  <MkOverlayHost />
</template>
