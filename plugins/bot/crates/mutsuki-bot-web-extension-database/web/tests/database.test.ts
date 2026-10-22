import { RpcError, type JsonValue } from "@mutsuki/web-sdk";
import { createFakeBridge } from "@mutsuki/web-sdk/testing";
import { flushPromises } from "@vue/test-utils";
import { afterEach, describe, expect, test } from "vitest";

import { formatCell } from "../src/cells";
import DatabasePage from "../src/DatabasePage.vue";
import { mountPage } from "./mount";

afterEach(() => {
  document.body.innerHTML = "";
});

const snapshot = {
  path: "state.sqlite3",
  journal_mode: "wal",
  size_bytes: 4096,
  tables: [
    {
      name: "bot_management_meta",
      row_count: 3,
      columns: [
        { name: "key", decl_type: "TEXT", primary_key: true },
        { name: "value", decl_type: "TEXT", primary_key: false },
      ],
    },
  ],
};

describe("cells", () => {
  test("format nulls, blobs, json and text", () => {
    expect(formatCell(null)).toEqual({ text: "NULL", kind: "null" });
    expect(formatCell({ $type: "blob", bytes: 12 })).toEqual({ text: "<blob 12 bytes>", kind: "blob" });
    expect(formatCell({ a: 1 })).toEqual({ text: '{"a":1}', kind: "json" });
    expect(formatCell(7)).toEqual({ text: "7", kind: "text" });
  });
});

describe("database page", () => {
  test("pages through a table by cursor and never renders stored markup", async () => {
    const pages: Record<string, JsonValue> = {
      "": {
        table: "bot_management_meta",
        columns: ["key", "value"],
        rows: [["schema", '"><img src=x onerror=alert(1)>']],
        next_cursor: "c1",
      },
      c1: { table: "bot_management_meta", columns: ["key", "value"], rows: [["revision", null]], next_cursor: null },
    };
    const bridge = createFakeBridge({
      handlers: {
        "database.snapshot": () => snapshot,
        "database.rows": (params) => pages[(params as { after: string }).after],
      },
    });
    const { wrapper } = mountPage(DatabasePage, { bridge });
    await flushPromises();
    expect(wrapper.text()).toContain("4 KiB");
    await wrapper.find(".database-table").trigger("click");
    await flushPromises();
    expect(wrapper.find("thead").text()).toContain("key *");
    expect(wrapper.find("tbody").text()).toContain('"><img src=x onerror=alert(1)>');
    expect(document.querySelector("img")).toBeNull();

    const button = (label: string) => wrapper.findAll("button").find((item) => item.text() === label)!;
    await button("下一页").trigger("click");
    await flushPromises();
    expect(wrapper.find("tbody").text()).toContain("NULL");
    expect(bridge.calls.at(-1)?.params).toEqual({ table: "bot_management_meta", after: "c1", limit: 50 });
    expect(button("下一页").attributes("disabled")).toBeDefined();
    await button("上一页").trigger("click");
    await flushPromises();
    expect(wrapper.find("tbody").text()).toContain("schema");
    wrapper.unmount();
  });

  test("an unbound console says so; a failed table read is retryable", async () => {
    const unbound = mountPage(DatabasePage, {
      bridge: createFakeBridge({ handlers: { "database.snapshot": () => null } }),
    });
    await flushPromises();
    expect(unbound.wrapper.text()).toContain("当前没有已接入的数据库");
    unbound.wrapper.unmount();

    let fail = true;
    const bridge = createFakeBridge({
      handlers: {
        "database.snapshot": () => snapshot,
        "database.rows": () => {
          if (fail) throw new RpcError("bot.database.unknown_table", "unknown table");
          return { table: "bot_management_meta", columns: ["key"], rows: [], next_cursor: null };
        },
      },
    });
    const { wrapper } = mountPage(DatabasePage, { bridge });
    await flushPromises();
    await wrapper.find(".database-table").trigger("click");
    await flushPromises();
    expect(wrapper.find('[role="alert"]').text()).toContain("unknown table");
    fail = false;
    await wrapper.find('[role="alert"] button').trigger("click");
    await flushPromises();
    expect(wrapper.text()).toContain("此表暂无数据");
    wrapper.unmount();
  });
});
