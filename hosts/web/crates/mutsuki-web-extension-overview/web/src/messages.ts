import { defineMessages } from "@mutsuki/web-sdk";

export const messages = defineMessages({
  "zh-CN": {
    title: "概览",
    refresh: "刷新",
    refreshFailed: "更新失败，将自动重试",
    uptime: "运行时间",
    status: "运行状态",
    plugins: "插件",
    runners: "运行器",
    eventSources: "事件源",
    health: { ok: "正常", degraded: "降级", failed: "异常", stopped: "已停止", unknown: "—" },
  },
});
