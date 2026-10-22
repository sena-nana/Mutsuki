import { defineMessages, useI18n } from "@mutsuki/web-sdk";

/** Built-in copy of `@mutsuki/ui`. The shell may register it as the `ui` i18n namespace. */
export const uiMessages = defineMessages({
  "zh-CN": {
    loading: "加载中…",
    retry: "重试",
    loadFailed: "加载失败",
    empty: "暂无数据",
    copy: "复制",
    copied: "已复制",
    copyFailed: "复制失败",
    close: "关闭",
    confirm: "确认",
    cancel: "取消",
    ok: "确定",
    clear: "清除",
    search: "搜索",
    show: "显示",
    hide: "隐藏",
    secretConfigured: "已配置（留空表示不修改）",
    typeToConfirm: "输入 {text} 以确认",
    skipToContent: "跳到主要内容",
    more: "更多操作",
    expand: "展开",
    collapse: "折叠",
    sortAscending: "升序",
    sortDescending: "降序",
    resize: "调整宽度",
    errorCode: "错误码：{code}",
    notifications: "通知",
    ledger: {
      older: "加载更早",
      loadingOlder: "正在加载…",
      search: "搜索记录",
      loading: "加载中",
      lost: "丢失 {count}",
      truncated: "已截断",
      count: "{visible} / {total} 条",
      empty: "暂无记录",
      inspect: "选择一条记录查看详情",
      status: "状态",
      running: "进行中",
      done: "完成",
      error: "失败",
      duration: "耗时 {duration}",
      input: "输入",
      output: "输出",
      thinking: "思考",
      raw: "原始事件",
      rawLoading: "正在加载完整事件…",
      label: "事件记录",
    },
  },
});

type Leaves<T, P extends string = ""> = {
  [K in keyof T & string]: T[K] extends string ? `${P}${K}` : Leaves<T[K], `${P}${K}.`>;
}[keyof T & string];
type UiKey = Leaves<(typeof uiMessages)["zh-CN"]>;

/**
 * `ui.*` copy from the shell's i18n when it registered the namespace, else the built-in
 * zh-CN text, so components never render raw keys.
 */
export function useUiText(): (key: UiKey, params?: Record<string, string | number>) => string {
  const i18n = useI18n();
  return (key, params) => {
    const scoped = `ui.${key}`;
    if (i18n.has(scoped)) return i18n.t(scoped, params);
    const template = key
      .split(".")
      .reduce<unknown>((node, part) => (node as Record<string, unknown>)?.[part], uiMessages["zh-CN"]) as string;
    return params
      ? template.replace(/\{(\w+)\}/g, (match, name: string) =>
          name in params ? String(params[name]) : match,
        )
      : template;
  };
}
