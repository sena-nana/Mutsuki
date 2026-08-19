function formatDuration(ms) {
  if (ms == null || Number.isNaN(Number(ms))) return "—";
  const total = Math.max(0, Math.floor(Number(ms) / 1000));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  if (hours) return `${hours} 小时 ${minutes} 分`;
  if (minutes) return `${minutes} 分 ${seconds} 秒`;
  return `${seconds} 秒`;
}

function healthLabel(value) {
  switch (String(value || "").toLowerCase()) {
    case "ok":
    case "healthy":
      return "正常";
    case "degraded":
      return "降级";
    case "unhealthy":
    case "failed":
      return "异常";
    case "stopped":
      return "已停止";
    default:
      return value == null || value === "" ? "—" : String(value);
  }
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;");
}

function metricGrid(metrics) {
  return `<div class="metric-grid">${metrics
    .map(
      ({ label, value }) =>
        `<div class="metric-card"><div class="metric-label">${escapeHtml(label)}</div><div class="metric-value">${escapeHtml(value)}</div></div>`,
    )
    .join("")}</div>`;
}

function mountOverview(host, ctx) {
  const { rpc, events } = ctx;
  host.className = "page-body overview-dashboard";
  host.innerHTML = `
    <div data-overview-body><div class="muted">加载中…</div></div>
    <div class="overview-cards" data-overview-cards></div>
  `;
  const body = host.querySelector("[data-overview-body]");
  const cardsHost = host.querySelector("[data-overview-cards]");
  const refreshButton = document.createElement("button");
  refreshButton.type = "button";
  refreshButton.className = "ghost console-icon-btn";
  refreshButton.setAttribute("aria-label", "刷新");
  refreshButton.innerHTML =
    '<svg aria-hidden="true" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-2.6-6.4M21 3v6h-6"/></svg>';
  const headerActions = host.closest(".console-main")?.querySelector(".console-page-header__actions");
  headerActions?.append(refreshButton);
  let disposed = false;
  let timer = null;
  let debounceTimer = null;
  let uptimeTimer = null;
  let inFlight = null;
  let pending = false;
  let lastRevision = 0;
  let opened = false;
  let snapshot = null;
  let snapshotAt = 0;
  const cardPanels = [];

  const render = () => {
    if (!snapshot || disposed) return;
    const data = snapshot;
    const health = data.health || {};
    const counts = data.counts || {};
    const uptime = Number(data.uptime_ms || 0) + Math.max(0, Date.now() - snapshotAt);
    body.innerHTML = `${metricGrid([
      { label: "运行时间", value: formatDuration(uptime) },
      { label: "服务状态", value: healthLabel(health.service) },
      { label: "消息处理", value: healthLabel(health.core) },
      { label: "插件", value: String(counts.plugins ?? 0) },
      { label: "运行器", value: String(counts.runners ?? 0) },
      { label: "事件源", value: String(counts.event_sources ?? 0) },
    ])}`;
  };

  const schedule = () => {
    clearTimeout(timer);
    if (!disposed && !document.hidden) timer = setTimeout(refresh, 60_000);
  };
  const refresh = () => {
    if (disposed) return Promise.resolve();
    if (inFlight) {
      pending = true;
      return inFlight;
    }
    inFlight = rpc
      .read("overview", "summary")
      .then((data) => {
        if (disposed) return;
        snapshot = data;
        snapshotAt = Date.now();
        render();
        refreshButton.removeAttribute("title");
      })
      .catch(() => {
        if (!disposed) refreshButton.title = "更新失败，将自动重试";
      })
      .finally(() => {
        inFlight = null;
        if (pending) {
          pending = false;
          void refresh();
        } else {
          schedule();
        }
      });
    return inFlight;
  };
  const visibility = () => {
    clearTimeout(timer);
    if (!document.hidden) void refresh();
  };
  const eventSubscription = events.subscribe("control.changed", (payload) => {
    const revision = Number(payload?.revision || 0);
    if (revision <= lastRevision) return;
    lastRevision = revision;
    if (document.hidden) return;
    clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => void refresh(), 50);
  }, "runtime.read");
  const connectionSubscription = events.onStateChange?.((connection) => {
    if (connection !== "open" || document.hidden) return;
    if (opened) void refresh();
    opened = true;
  });
  refreshButton.onclick = () => void refresh();
  document.addEventListener("visibilitychange", visibility);
  uptimeTimer = setInterval(render, 1_000);
  for (const item of ctx.slots.list().filter((entry) => entry.slot === "overview.cards")) {
    const node = document.createElement("div");
    cardsHost.append(node);
    const mounted = item.component?.mount?.(node);
    if (mounted) cardPanels.push(mounted);
  }
  void refresh();
  return {
    dispose() {
      disposed = true;
      refreshButton.remove();
      for (const panel of cardPanels) {
        panel.dispose?.() ?? panel.destroy?.();
      }
      clearTimeout(timer);
      clearTimeout(debounceTimer);
      clearInterval(uptimeTimer);
      eventSubscription.dispose();
      connectionSubscription?.dispose();
      document.removeEventListener("visibilitychange", visibility);
    },
  };
}

export default {
  id: "overview",
  setup(ctx) {
    ctx.navigation.register({
      id: "overview.nav",
      activityId: "home",
      pageId: "overview.page",
      label: "概览",
      order: 1,
      requiredCapability: "runtime.read",
    });
    ctx.pages.register({
      id: "overview.page",
      path: "/overview",
      title: "概览",
      component: { mount: (element) => mountOverview(element, ctx) },
      requiredCapability: "runtime.read",
    });
  },
};
