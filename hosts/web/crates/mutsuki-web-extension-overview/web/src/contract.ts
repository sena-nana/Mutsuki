import { defineRpc, query, topic } from "@mutsuki/web-sdk";

export interface OverviewSummary {
  uptime_ms?: number | null;
  health?: { service?: string | null; core?: string | null } | null;
  counts?: {
    plugins?: number | null;
    runners?: number | null;
    event_sources?: number | null;
    tasks?: { active?: number | null } | number | null;
  } | null;
}

/** `overview.*` RPCs of `mutsuki-web-extension-overview` (hand-written until B7). */
export const overviewApi = defineRpc({
  namespace: "overview",
  methods: {
    summary: query<void, OverviewSummary>({ capability: "runtime.read" }),
  },
});

/** Published by the console's control-change bridge on every runtime revision. */
export const controlChanged = topic<{ revision?: number; domains?: string[] }>(
  "control.changed",
  "runtime.read",
);
