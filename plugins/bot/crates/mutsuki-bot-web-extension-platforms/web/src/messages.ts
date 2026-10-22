import { defineMessages } from "@mutsuki/web-sdk";

export const messages = defineMessages({
  "zh-CN": {
    title: "账号",
    refresh: "刷新",
    noAdapters: "尚未启用任何平台适配器",
    noAdaptersHint: "在插件里启用 QQ、B 站等平台后，账号会出现在这里。",
    noAccounts: "该平台还没有账号",
    accounts: "平台账号",
    select: "选择左侧的账号查看详情",
    configure: "平台配置",
    lastError: "最近错误",
    actions: "操作",
    actionConfirm: "确认对 {account} 执行「{action}」？",
    actionDone: "「{action}」已完成",
    actionFailed: "「{action}」失败：{message}",
    disabled: "已停用",
    health: { ok: "正常", degraded: "降级", unhealthy: "异常", stopped: "已停止" },
    secret: { configured: "已配置", missing: "未配置", invalid: "无效", restricted: "无权查看" },
    yes: "是",
    no: "否",
    unknown: "—",
  },
});
