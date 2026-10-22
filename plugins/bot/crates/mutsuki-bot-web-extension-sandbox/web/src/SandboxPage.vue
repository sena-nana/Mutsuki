<script setup lang="ts">
import { MkErrorState, MkSkeleton } from "@mutsuki/ui";
import { useMessages } from "@mutsuki/web-sdk";

import ChatPane from "./ChatPane.vue";
import MemberPanel from "./MemberPanel.vue";
import { messages } from "./messages";
import SessionList from "./SessionList.vue";
import { createSandboxStore, provideSandboxStore } from "./store";

/** Three panes: conversations, the chat with its composer, and members / identity. */
const store = createSandboxStore();
provideSandboxStore(store);
const { t } = useMessages(messages);
</script>

<template>
  <div class="page-body">
    <MkErrorState
      v-if="store.snapshot.error.value && !store.data.value"
      :title="t('loadFailed')"
      :error="store.snapshot.error.value"
      @retry="store.snapshot.refresh()"
    />
    <MkSkeleton v-else-if="!store.data.value" variant="block" />
    <div v-else class="sandbox-client">
      <div class="sandbox-frame">
        <SessionList />
        <ChatPane />
        <MemberPanel />
      </div>
    </div>
  </div>
</template>
