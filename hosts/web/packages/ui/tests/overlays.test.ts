import { mount } from "@vue/test-utils";
import { afterEach, describe, expect, test, vi } from "vitest";
import { defineComponent, h } from "vue";

import {
  MAX_TOASTS,
  MkOverlayHost,
  installOverlays,
  useConfirm,
  usePrompt,
  useToast,
  type Toaster,
} from "../src/index";
import { axe, settle } from "./helpers";

afterEach(() => {
  document.body.innerHTML = "";
  vi.useRealTimers();
});

function mountHost() {
  let api!: { confirm: ReturnType<typeof useConfirm>; prompt: ReturnType<typeof usePrompt>; toast: Toaster };
  const App = defineComponent({
    setup() {
      api = { confirm: useConfirm(), prompt: usePrompt(), toast: useToast() };
      return () => h(MkOverlayHost);
    },
  });
  const wrapper = mount(App, {
    attachTo: document.body,
    global: { plugins: [{ install: (app) => void installOverlays(app) }] },
  });
  return { wrapper, api };
}

function button(label: string): HTMLButtonElement {
  const found = [...document.body.querySelectorAll<HTMLButtonElement>(".mk-dialog button")].find(
    (candidate) => candidate.textContent?.trim() === label,
  );
  if (!found) throw new Error(`no button ${label}`);
  return found;
}

describe("useConfirm", () => {
  test("requireText blocks confirmation until the exact text is typed", async () => {
    const { wrapper, api } = mountHost();
    const result = api.confirm({ title: "删除数据库", danger: true, requireText: "main", confirmLabel: "删除" });
    await settle();
    expect(document.body.querySelector('[role="alertdialog"]')).not.toBeNull();
    expect(button("删除").disabled).toBe(true);
    const input = document.body.querySelector<HTMLInputElement>(".mk-dialog input")!;
    input.value = "mai";
    input.dispatchEvent(new Event("input"));
    await settle();
    expect(button("删除").disabled).toBe(true);
    input.value = "main";
    input.dispatchEvent(new Event("input"));
    await settle();
    expect(await axe(document.body)).toHaveNoViolations();
    button("删除").click();
    await expect(result).resolves.toBe(true);
    await settle();
    expect(document.body.querySelector(".mk-dialog")).toBeNull();
    wrapper.unmount();
  });

  test("cancel and Escape resolve false; requests are shown one at a time", async () => {
    const { wrapper, api } = mountHost();
    const first = api.confirm({ title: "第一个" });
    const second = api.confirm({ title: "第二个" });
    await settle();
    expect(document.body.querySelector(".mk-dialog__title")?.textContent).toBe("第一个");
    button("取消").click();
    await expect(first).resolves.toBe(false);
    await settle();
    expect(document.body.querySelector(".mk-dialog__title")?.textContent).toBe("第二个");
    document.body
      .querySelector(".mk-dialog__panel")!
      .dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await expect(second).resolves.toBe(false);
    wrapper.unmount();
  });
});

describe("usePrompt", () => {
  test("validates before resolving and resolves null on cancel", async () => {
    const { wrapper, api } = mountHost();
    const result = api.prompt({
      title: "重命名",
      label: "名称",
      initial: "",
      validate: (value) => (value.trim() ? null : "名称不能为空"),
    });
    await settle();
    button("确定").click();
    await settle();
    expect(document.body.querySelector(".mk-field__error")?.textContent).toBe("名称不能为空");
    const input = document.body.querySelector<HTMLInputElement>(".mk-dialog input")!;
    expect(input.getAttribute("aria-invalid")).toBe("true");
    input.value = "新名称";
    input.dispatchEvent(new Event("input"));
    await settle();
    button("确定").click();
    await expect(result).resolves.toBe("新名称");

    const cancelled = api.prompt({ title: "重命名", label: "名称" });
    await settle();
    button("取消").click();
    await expect(cancelled).resolves.toBeNull();
    wrapper.unmount();
  });
});

describe("useToast", () => {
  test("errors persist as alerts, others expire, and the stack is bounded", async () => {
    vi.useFakeTimers();
    const { wrapper, api } = mountHost();
    api.toast.error("投递失败");
    api.toast.success("已保存");
    await vi.advanceTimersByTimeAsync(0);
    const toasts = () => [...document.body.querySelectorAll(".mk-toast")];
    expect(toasts().map((toast) => toast.getAttribute("role"))).toEqual(["alert", "status"]);
    await vi.advanceTimersByTimeAsync(5_000);
    expect(toasts().map((toast) => toast.textContent?.trim())).toEqual(["投递失败"]);
    for (let index = 0; index < MAX_TOASTS + 2; index += 1) api.toast.info(`消息 ${index}`);
    await vi.advanceTimersByTimeAsync(0);
    expect(toasts()).toHaveLength(MAX_TOASTS);
    expect(toasts()[0].textContent).not.toContain("投递失败");
    wrapper.unmount();
  });
});
