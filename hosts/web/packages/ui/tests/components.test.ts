import { mount } from "@vue/test-utils";
import { afterEach, describe, expect, test, vi } from "vitest";
import { defineComponent, h, nextTick, ref } from "vue";

import {
  MkAsync,
  MkButton,
  MkCard,
  MkCodeBlock,
  MkDataTable,
  MkDialog,
  MkEmptyState,
  MkErrorState,
  MkField,
  MkIconButton,
  MkJsonView,
  MkKeyValue,
  MkLiveRegion,
  MkMenu,
  MkNumberInput,
  MkPageHeader,
  MkRadioGroup,
  MkSecretInput,
  MkSection,
  MkSegmented,
  MkSelect,
  MkSkipLink,
  MkSplitPane,
  MkStatusBadge,
  MkSwitch,
  MkTabs,
  MkTextInput,
  MkTimeline,
  uiMessages,
} from "../src/index";
import { checkMessageKeys } from "@mutsuki/web-sdk";
import { axe, settle } from "./helpers";

afterEach(() => {
  document.body.innerHTML = "";
});

describe("accessibility", () => {
  test("a page assembled from Mk components has no axe violations", async () => {
    const Page = defineComponent({
      setup() {
        const tab = ref("a");
        const text = ref("");
        const secret = ref("");
        const mode = ref<"light" | "dark">("light");
        const choice = ref<string | null>("x");
        const amount = ref<number | null>(3);
        const enabled = ref(true);
        const split = ref(40);
        return () =>
          h("main", { id: "main" }, [
            h(MkSkipLink),
            h(MkPageHeader, { title: "插件", description: "管理已加载的插件" }, {
              actions: () => h(MkButton, { variant: "primary", icon: "refresh" }, () => "刷新"),
            }),
            h(MkSection, { title: "概况" }, () => [
              h(MkKeyValue, { items: [{ label: "版本", value: "1.0" }, { label: "实例", value: null }] }),
              h(MkStatusBadge, { tone: "ok", label: "正常" }),
              h(MkTimeline, { label: "事件", items: [{ id: "1", title: "启动", tone: "ok" }] }),
            ]),
            h(MkCard, { title: "表格" }, () =>
              h(MkDataTable, {
                caption: "插件列表",
                rowKey: "id",
                columns: [
                  { key: "id", label: "ID", sortable: true },
                  { key: "count", label: "数量", align: "end" },
                ],
                rows: [{ id: "a", count: 1 }],
              }),
            ),
            h(MkTabs, { label: "视图", tabs: [{ id: "a", label: "甲" }, { id: "b", label: "乙" }], modelValue: tab.value, "onUpdate:modelValue": (v: string) => (tab.value = v) }, {
              a: () => h("p", "甲内容"),
              b: () => h("p", "乙内容"),
            }),
            h(MkField, { label: "名称", hint: "显示名", error: "不能为空", required: true }, () =>
              h(MkTextInput, { modelValue: text.value, "onUpdate:modelValue": (v: string) => (text.value = v) }),
            ),
            h(MkField, { label: "密钥" }, () => h(MkSecretInput, { configured: true, modelValue: secret.value })),
            h(MkField, { label: "数量" }, () => h(MkNumberInput, { modelValue: amount.value })),
            h(MkField, { label: "选项" }, () =>
              h(MkSelect, { options: [{ value: "x", label: "X" }], modelValue: choice.value }),
            ),
            h(MkField, { label: "启用" }, () => h(MkSwitch, { modelValue: enabled.value })),
            h(MkRadioGroup, { legend: "级别", options: [{ value: "x", label: "X" }, { value: "y", label: "Y" }], modelValue: choice.value }),
            h(MkSegmented, { label: "主题", options: [{ value: "light", label: "浅色" }, { value: "dark", label: "深色" }], modelValue: mode.value }),
            h(MkSplitPane, { modelValue: split.value }, { start: () => h("p", "左"), end: () => h("p", "右") }),
            h(MkJsonView, { label: "载荷", value: { a: [1, "x", null] } }),
            h(MkCodeBlock, { code: "cargo test", language: "bash" }),
            h(MkIconButton, { icon: "more", label: "更多" }),
            h(MkEmptyState, { title: "暂无插件" }),
            h(MkErrorState, { error: { code: "capability_denied", message: "缺少权限" } }),
            h(MkLiveRegion, { message: "已连接" }),
          ]);
      },
    });
    const wrapper = mount(Page, { attachTo: document.body });
    await settle();
    expect(await axe(document.body)).toHaveNoViolations();
    wrapper.unmount();
  });

  test("an open dialog is an accessible modal", async () => {
    const wrapper = mount(MkDialog, {
      props: { open: true, title: "删除插件", description: "此操作不可撤销", alert: true },
      slots: { default: "<p>确认删除？</p>", footer: "<button type='button'>删除</button>" },
      attachTo: document.body,
    });
    await settle();
    const dialog = document.body.querySelector('[role="alertdialog"]')!;
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    expect(document.getElementById(dialog.getAttribute("aria-labelledby")!)?.textContent).toBe(
      "删除插件",
    );
    expect(await axe(document.body)).toHaveNoViolations();
    wrapper.unmount();
  });
});

describe("behavior", () => {
  test("tabs move with arrow keys and expose the selected panel", async () => {
    const active = ref("a");
    const wrapper = mount(MkTabs, {
      props: {
        label: "视图",
        tabs: [{ id: "a", label: "甲" }, { id: "b", label: "乙", disabled: true }, { id: "c", label: "丙" }],
        modelValue: active.value,
        "onUpdate:modelValue": (value: string) => {
          active.value = value;
          void wrapper.setProps({ modelValue: value });
        },
      },
      slots: { a: "<p>A</p>", c: "<p>C</p>" },
      attachTo: document.body,
    });
    await wrapper.find('[role="tablist"]').trigger("keydown", { key: "ArrowRight" });
    expect(active.value).toBe("c");
    await nextTick();
    const tabs = wrapper.findAll('[role="tab"]');
    expect(tabs[2].attributes("aria-selected")).toBe("true");
    expect(tabs[2].attributes("tabindex")).toBe("0");
    expect(wrapper.find('[role="tabpanel"]:not([style*="display: none"])').text()).toBe("C");
    await wrapper.find('[role="tablist"]').trigger("keydown", { key: "Home" });
    expect(active.value).toBe("a");
    wrapper.unmount();
  });

  test("data table sorts by column and announces the order", async () => {
    const wrapper = mount(MkDataTable, {
      props: {
        caption: "列表",
        rowKey: "id",
        columns: [{ key: "id", label: "ID", sortable: true }],
        rows: [{ id: "b" }, { id: "a" }, { id: "c" }],
      },
    });
    const cells = () => wrapper.findAll("tbody td").map((cell) => cell.text());
    expect(cells()).toEqual(["b", "a", "c"]);
    await wrapper.find(".mk-table__sort").trigger("click");
    expect(cells()).toEqual(["a", "b", "c"]);
    expect(wrapper.find("th").attributes("aria-sort")).toBe("ascending");
    await wrapper.find(".mk-table__sort").trigger("click");
    expect(cells()).toEqual(["c", "b", "a"]);
    expect(wrapper.find("th").attributes("aria-sort")).toBe("descending");
    await wrapper.setProps({ rows: [] });
    expect(wrapper.find(".mk-table__empty").text()).toBe("暂无数据");
  });

  test("field wires label, description and invalid state into its control", () => {
    const wrapper = mount(MkField, {
      props: { label: "名称", hint: "显示名", error: "必填", required: true },
      slots: { default: () => h(MkTextInput) },
    });
    const input = wrapper.find("input");
    const label = wrapper.find("label");
    expect(label.attributes("for")).toBe(input.attributes("id"));
    const describedBy = input.attributes("aria-describedby")!.split(" ");
    expect(describedBy.map((id) => wrapper.find(`#${id}`).text())).toEqual(["显示名", "必填"]);
    expect(input.attributes("aria-invalid")).toBe("true");
    expect(input.attributes("required")).toBeDefined();
  });

  test("secret input never echoes the stored secret and toggles visibility", async () => {
    const wrapper = mount(MkSecretInput, { props: { configured: true, label: "密钥" } });
    const input = wrapper.find("input");
    expect(input.element.value).toBe("");
    expect(input.attributes("type")).toBe("password");
    expect(input.attributes("placeholder")).toContain("已配置");
    await wrapper.find("button").trigger("click");
    expect(wrapper.find("input").attributes("type")).toBe("text");
    expect(wrapper.find("button").attributes("aria-pressed")).toBe("true");
  });

  test("segmented control selects with arrow keys", async () => {
    const value = ref("light");
    const wrapper = mount(MkSegmented, {
      props: {
        label: "主题",
        options: [
          { value: "system", label: "跟随系统" },
          { value: "light", label: "浅色" },
          { value: "dark", label: "深色" },
        ],
        modelValue: value.value,
        "onUpdate:modelValue": (next: string | number) => (value.value = String(next)),
      },
      attachTo: document.body,
    });
    await wrapper.find('[role="radiogroup"]').trigger("keydown", { key: "ArrowRight" });
    expect(value.value).toBe("dark");
    wrapper.unmount();
  });

  test("number input maps empty text to null", async () => {
    const updates: Array<number | null> = [];
    const wrapper = mount(MkNumberInput, {
      props: { label: "数量", modelValue: 3, "onUpdate:modelValue": (v: number | null) => updates.push(v) },
    });
    await wrapper.find("input").setValue("");
    await wrapper.find("input").setValue("12");
    expect(updates).toEqual([null, 12]);
  });

  test("split pane resizes from the keyboard within bounds", async () => {
    const sizes: number[] = [];
    const wrapper = mount(MkSplitPane, {
      props: { modelValue: 20, min: 15, max: 85, "onUpdate:modelValue": (v: number) => sizes.push(v) },
    });
    const divider = wrapper.find('[role="separator"]');
    await divider.trigger("keydown", { key: "ArrowLeft", shiftKey: true });
    await divider.trigger("keydown", { key: "End" });
    expect(sizes).toEqual([15, 85]);
  });

  test("async shows loading, error with retry, empty and data", async () => {
    const refresh = vi.fn();
    const source = {
      data: ref<string[] | undefined>(undefined),
      error: ref<{ code: string; message: string } | null>(null),
      loading: ref(true),
      refresh,
    };
    const wrapper = mount(MkAsync, {
      props: { source },
      slots: {
        default: ({ data }: { data: unknown }) =>
          h("ul", (data as string[]).map((item) => h("li", item))),
      },
    });
    expect(wrapper.find('[role="status"]').text()).toBe("加载中…");
    source.loading.value = false;
    source.error.value = { code: "disconnected", message: "连接断开" };
    await nextTick();
    expect(wrapper.find('[role="alert"]').text()).toContain("disconnected");
    await wrapper.find("button").trigger("click");
    expect(refresh).toHaveBeenCalledOnce();
    source.error.value = null;
    source.data.value = [];
    await nextTick();
    expect(wrapper.text()).toContain("暂无数据");
    source.data.value = ["a", "b"];
    await nextTick();
    expect(wrapper.findAll("li").map((item) => item.text())).toEqual(["a", "b"]);
  });

  test("dialog closes on Escape unless persistent and traps focus", async () => {
    const wrapper = mount(MkDialog, {
      props: { open: true, title: "标题", "onUpdate:open": (open: boolean) => wrapper.setProps({ open }) },
      slots: { default: "<input aria-label='first' />", footer: "<button type='button'>最后</button>" },
      attachTo: document.body,
    });
    await settle();
    const panel = document.body.querySelector<HTMLElement>(".mk-dialog__panel")!;
    expect(panel.contains(document.activeElement)).toBe(true);
    const last = panel.querySelector<HTMLButtonElement>(".mk-dialog__footer button")!;
    last.focus();
    last.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", bubbles: true }));
    expect(panel.contains(document.activeElement)).toBe(true);
    expect(document.activeElement).not.toBe(last);

    await wrapper.setProps({ persistent: true });
    panel.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await nextTick();
    expect(document.body.querySelector(".mk-dialog")).not.toBeNull();
    await wrapper.setProps({ persistent: false });
    document.body
      .querySelector(".mk-dialog__panel")!
      .dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await settle();
    expect(document.body.querySelector(".mk-dialog")).toBeNull();
    wrapper.unmount();
  });

  test("menu opens from the keyboard, selects and returns focus", async () => {
    const selected: string[] = [];
    const wrapper = mount(MkMenu, {
      props: {
        label: "插件操作",
        items: [
          { id: "reload", label: "重载" },
          { id: "disable", label: "停用", disabled: true },
          { id: "remove", label: "删除", danger: true },
        ],
        onSelect: (id: string) => selected.push(id),
      },
      slots: {
        trigger: ({ attrs, toggle }: { attrs: Record<string, unknown>; toggle: () => void }) =>
          h("button", { type: "button", ...attrs, onClick: toggle }, "操作"),
      },
      attachTo: document.body,
    });
    const trigger = wrapper.find("button");
    await trigger.trigger("keydown", { key: "ArrowDown" });
    await settle();
    expect(trigger.attributes("aria-expanded")).toBe("true");
    expect(document.activeElement?.textContent?.trim()).toBe("重载");
    await wrapper.find('[role="menu"]').trigger("keydown", { key: "ArrowDown" });
    expect(document.activeElement?.textContent?.trim()).toBe("删除");
    (document.activeElement as HTMLButtonElement).click();
    await settle();
    expect(selected).toEqual(["remove"]);
    expect(wrapper.find('[role="menu"]').exists()).toBe(false);
    expect(document.activeElement).toBe(trigger.element);
    wrapper.unmount();
  });

  test("ui messages are complete for every locale", () => {
    expect(checkMessageKeys(uiMessages)).toEqual([]);
  });
});
