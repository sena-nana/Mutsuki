// Generated from the Rust contract markers; do not edit.
// Regenerate with MUTSUKI_BLESS_TS_CONTRACT=1 cargo test.
import { defineRpc, mutation, query, topic } from "@mutsuki/web-sdk";
import type { AccountActionParams, AccountActionResult, AccountsListParams, NoParams, PlatformAccountView, PlatformAdapterView, PlatformsChanged } from "../types";

export const platformsApi = defineRpc({
  namespace: "platforms",
  methods: {
    "adapters.list": query<NoParams, PlatformAdapterView[]>({ capability: "bot.platforms.read" }),
    "accounts.list": query<AccountsListParams, PlatformAccountView[]>({ capability: "bot.platforms.read" }),
    "accounts.action": mutation<AccountActionParams, AccountActionResult>({ capability: "bot.platforms.write" }),
  },
});

export const changedTopic = topic<PlatformsChanged>("platforms.changed", "bot.platforms.read");
