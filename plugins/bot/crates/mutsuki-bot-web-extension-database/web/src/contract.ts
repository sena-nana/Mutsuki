import { defineRpc, query, type JsonValue } from "@mutsuki/web-sdk";

export interface DatabaseColumn {
  name: string;
  decl_type: string;
  primary_key: boolean;
}

export interface DatabaseTable {
  name: string;
  row_count: number;
  columns: DatabaseColumn[];
}

export interface DatabaseSnapshot {
  path: string;
  journal_mode: string;
  size_bytes?: number | null;
  tables: DatabaseTable[];
}

export interface DatabasePage {
  table: string;
  columns: string[];
  rows: JsonValue[][];
  next_cursor?: string | null;
}

/** `database.*` RPCs of `mutsuki-bot-web-extension-database` (hand-written until B7 types it). */
export const databaseApi = defineRpc({
  namespace: "database",
  methods: {
    /** `null` when no state database is bound to the console. */
    snapshot: query<void, DatabaseSnapshot | null>({ capability: "runtime.read" }),
    rows: query<{ table: string; after: string; limit: number }, DatabasePage>({
      capability: "runtime.read",
    }),
  },
});

export const PAGE_SIZE = 50;
