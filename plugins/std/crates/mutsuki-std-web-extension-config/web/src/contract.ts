import { defineRpc, mutation, query, topic } from "@mutsuki/web-sdk";

import type { ApplyResult, ConfigContext, ConfigDescriptor, ConfigSnapshot, ValidationResult, WireValue } from "./schema";

export interface NavigationGroup {
  label?: string;
  items: Array<{ provider_id: string; label?: string }>;
}

export interface ConfigMetrics {
  config_provider_count: number;
  config_revision_conflict: number;
  config_apply_failed: number;
  config_apply_latency_avg_ms: number;
  config_reload_required: number;
}

const schemaRead = { capability: "config.schema.read" };

/** `config.*` RPCs of `mutsuki-std-web-extension-config`. Errors carry `config.<kind>` codes. */
export const configApi = defineRpc({
  namespace: "config",
  methods: {
    "providers.list": query<void, unknown[]>(schemaRead),
    "navigation.list": query<void, NavigationGroup[]>(schemaRead),
    "schema.get": query<{ provider_id: string }, ConfigDescriptor>(schemaRead),
    "snapshot.read": query<{ provider_id: string; context: ConfigContext }, ConfigSnapshot>({ capability: "config.value.read" }),
    validate: mutation<{ provider_id: string; candidate: WireValue; context: ConfigContext }, ValidationResult>({
      capability: "config.value.write",
    }),
    apply: mutation<
      {
        provider_id: string;
        context: ConfigContext;
        request: { candidate: WireValue; expected_revision: number; dry_run: boolean };
      },
      ApplyResult
    >({ capability: "config.apply" }),
    metrics: query<void, ConfigMetrics>(schemaRead),
  },
});

export const revisionChanged = topic<{ provider_id?: unknown; revision?: number | { value?: number } }>(
  "config.revision_changed",
  "config.schema.read",
);

export interface PluginEntry {
  plugin_id: string;
  configured?: boolean;
  active_deployment?: string | null;
  candidates?: Array<{ deployment: string; version?: string | null }>;
}

/** The one Control API read the hubs need: which plugins are loaded and at what version. */
export const controlPlugins = defineRpc({
  namespace: "control",
  methods: {
    plugin_list: query<void, { plugins: PluginEntry[] }>({ capability: "runtime.read" }),
  },
});
