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
  },
});

type UiKey = keyof (typeof uiMessages)["zh-CN"];

/**
 * `ui.*` copy from the shell's i18n when it registered the namespace, else the built-in
 * zh-CN text, so components never render raw keys.
 */
export function useUiText(): (key: UiKey, params?: Record<string, string | number>) => string {
  const i18n = useI18n();
  return (key, params) => {
    const scoped = `ui.${key}`;
    if (i18n.has(scoped)) return i18n.t(scoped, params);
    const template = uiMessages["zh-CN"][key];
    return params
      ? template.replace(/\{(\w+)\}/g, (match, name: string) =>
          name in params ? String(params[name]) : match,
        )
      : template;
  };
}
