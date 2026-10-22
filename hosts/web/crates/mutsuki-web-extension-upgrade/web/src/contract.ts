import { defineRpc, query } from "@mutsuki/web-sdk";

export interface UpgradeModule {
  id: string;
  current_revision?: string | null;
  remote_revision?: string | null;
}

export interface UpgradePlan {
  module_id: string;
  plan: unknown;
  cli_command?: string;
}

/** `upgrade.*` RPCs of `mutsuki-web-extension-upgrade` (hand-written until B7 generates them). */
export const upgradeApi = defineRpc({
  namespace: "upgrade",
  methods: {
    check: query<{ query: string }, { modules: UpgradeModule[] }>({ capability: "runtime.read" }),
    plan: query<{ module_id: string }, UpgradePlan>({ capability: "runtime.read" }),
  },
});
