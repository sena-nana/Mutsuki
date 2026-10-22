// TypeScript mirror of `mutsuki_bot_console_protocol::platforms`. The method table is generated
// (`generated/platforms.contract.gen.ts`); keep these DTOs in step with the Rust structs.
import type { JsonValue } from "@mutsuki/web-sdk";

export type NoParams = Record<string, never>;

export type PlatformAdapterKind = "messaging" | "service_account";

export type PlatformFieldFormat =
  | "text"
  | "mono"
  | "timestamp"
  | "since"
  | "count"
  | "status"
  | "secret_status";

export interface PlatformAccountField {
  key: string;
  label: string;
  format: PlatformFieldFormat;
}

export interface PlatformAccountAction {
  id: string;
  label: string;
  danger: boolean;
  capability: string;
}

export interface PlatformAdapterView {
  adapter_id: string;
  platform: string;
  label: string;
  kind: PlatformAdapterKind;
  config_provider_id?: string;
  account_fields: PlatformAccountField[];
  account_actions: PlatformAccountAction[];
  detail_panels: string[];
}

export type PlatformHealth = "ok" | "degraded" | "unhealthy" | "stopped";

export interface PlatformAccountError {
  code: string;
  message: string;
}

export interface PlatformAccountView {
  adapter_id: string;
  account_id: string;
  display_name: string;
  avatar_url?: string;
  enabled: boolean;
  health: PlatformHealth;
  connection_state: string;
  fields: Record<string, JsonValue>;
  last_error?: PlatformAccountError;
}

export interface AccountsListParams {
  adapter_id?: string;
}

export interface AccountActionParams {
  adapter_id: string;
  account_id: string;
  action: string;
  params?: JsonValue;
}

export interface AccountActionResult {
  message?: string;
  data: JsonValue;
}

export interface PlatformsChanged {
  adapter_id?: string;
}
