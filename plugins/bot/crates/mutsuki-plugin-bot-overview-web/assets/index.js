/**
 * Mutsuki Bot Web Console shell: overview, plugins, runners, event sources, logs.
 */

const READ_CAPS = ["runtime.read"];
const WRITE_CAPS = ["runtime.read", "runtime.write"];

const PAGE_ALIASES = {
  runners: { page: "runtime", tab: "runners" },
  events: { page: "runtime", tab: "events" },
  lifecycle: { page: "ops", tab: "lifecycle" },
  logs: { page: "ops", tab: "logs" },
};

const PAGES = [
  { id: "overview", label: "概览" },
  { id: "tasks", label: "任务" },
  { id: "qq-bot", label: "QQ 管理", optional: true },
  { id: "agent-connections", label: "Agent", optional: true },
  { id: "bot-flow", label: "流程编排", optional: true },
  { id: "config", label: "配置", optional: true },
];

const RUNTIME_TABS = [
  { id: "runners", label: "运行器" },
  { id: "events", label: "事件源" },
  { id: "topology", label: "拓扑" },
];

const OPS_TABS = [
  { id: "lifecycle", label: "生命周期" },
  { id: "logs", label: "日志" },
];

function formatDuration(ms) {
  if (ms == null || Number.isNaN(Number(ms))) return "—";
  const total = Math.max(0, Math.floor(Number(ms) / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  if (h > 0) return `${h} 小时 ${m} 分`;
  if (m > 0) return `${m} 分 ${s} 秒`;
  return `${s} 秒`;
}

function elapsed(startedAt) {
  if (startedAt == null) return null;
  return Math.max(0, Date.now() - Number(startedAt));
}

function healthClass(value) {
  const v = String(value || "").toLowerCase();
  if (v === "ok" || v === "healthy") return "ok";
  if (v === "degraded") return "warn";
  if (v === "unhealthy" || v === "stopped" || v === "failed") return "err";
  return "";
}

function healthLabel(value) {
  if (value == null || value === "") return "—";
  const v = String(value).toLowerCase();
  switch (v) {
    case "ok":
    case "healthy":
      return "正常";
    case "degraded":
      return "降级";
    case "unhealthy":
      return "异常";
    case "stopped":
      return "已停止";
    case "failed":
      return "失败";
    default:
      return String(value);
  }
}

function componentLabel(id) {
  switch (id) {
    case "distribution":
      return "分发";
    case "worker_pools":
      return "工作池";
    default:
      return id;
  }
}

function parseRoute() {
  const params = new URLSearchParams(location.search);
  let page = params.get("page") || "overview";
  let tab = params.get("tab") || "";
  const alias = PAGE_ALIASES[page];
  if (alias) {
    page = alias.page;
    tab = tab || alias.tab;
  }
  if (page === "runtime" && !RUNTIME_TABS.some((item) => item.id === tab)) {
    tab = "runners";
  }
  if (page === "ops" && !OPS_TABS.some((item) => item.id === tab)) {
    tab = "lifecycle";
  }
  return { page, tab };
}

function currentPage() {
  return parseRoute().page;
}

function currentTab() {
  return parseRoute().tab;
}

function navigate(page, tab) {
  const url = new URL(location.href);
  if (page === "overview") url.searchParams.delete("page");
  else url.searchParams.set("page", page);
  if (tab) url.searchParams.set("tab", tab);
  else url.searchParams.delete("tab");
  // Drop legacy alias params after normalize.
  history.pushState({}, "", url);
  return { page, tab: tab || "" };
}

function formatBytes(bytes) {
  if (bytes == null || Number.isNaN(Number(bytes))) return "—";
  const value = Number(bytes);
  if (value < 1024) return `${value} B`;
  if (value < 1024 ** 2) return `${(value / 1024).toFixed(1)} KB`;
  if (value < 1024 ** 3) return `${(value / (1024 ** 2)).toFixed(1)} MB`;
  return `${(value / (1024 ** 3)).toFixed(2)} GB`;
}

/** @param {unknown} err */
export function formatRpcError(err) {
  if (err && typeof err === "object" && "code" in err && err.code) {
    const code = String(err.code);
    const message = "message" in err && err.message ? String(err.message) : "操作失败";
    return `[${code}] ${message}`;
  }
  if (err instanceof Error) return err.message;
  return String(err);
}

function createShell(rpc, options = {}) {
  const includeConfig = options.includeConfig === true;
  const includeUpgrade = options.includeUpgrade === true;
  const includeBilibili = options.includeBilibili === true;
  const includeQq = options.includeQq === true;
  const includeAgentConnections = options.includeAgentConnections === true;
  const includeBotFlow = options.includeBotFlow === true;
  const builtinDatabases = Array.isArray(options.builtinDatabases) ? options.builtinDatabases : [];
  const setupOnly = includeConfig && !includeBilibili && !includeQq && !includeAgentConnections && !includeBotFlow;
  const pageAvailable = (page) => {
    if (page === "overview" || page === "tasks") return !setupOnly;
    if (page === "config") return includeConfig;
    if (page === "bilibili") return includeBilibili;
    if (page === "qq-bot") return includeQq;
    if (page === "agent-connections") return includeAgentConnections;
    if (page === "bot-flow") return includeBotFlow;
    return false;
  };
  const route = parseRoute();
  const state = {
    page: pageAvailable(route.page) ? route.page : (includeConfig ? "config" : "overview"),
    tab: route.tab,
    error: "",
    busy: false,
    upgradeDetail: null,
    upgradeQuery: "",
    selectedTaskId: null,
  };

  const app = document.createElement("div");
  app.className = "mutsuki-console lilia-workspace";
  app.dataset.liliaSurfaceMode = "solid";
  app.dataset.liliaSurfaceLevel = "base";

  function go(page, tab) {
    const destination = pageAvailable(page) ? page : (includeConfig ? "config" : "overview");
    const next = navigate(destination, tab);
    state.page = next.page;
    state.tab = next.tab;
    renderNav();
    renderPage();
  }

  function renderNav() {
    const nav = app.querySelector(".nav");
    nav.innerHTML = "";
    for (const page of PAGES) {
      if (!pageAvailable(page.id)) continue;
      const btn = document.createElement("button");
      btn.type = "button";
      const active = state.page === page.id;
      btn.className = `sb-tree__row lilia-interactive-item${active ? " is-active" : ""}`;
      btn.dataset.page = page.id;
      if (active) {
        btn.setAttribute("aria-current", "page");
        btn.dataset.liliaSelected = "true";
      }
      btn.innerHTML = `<span class="sb-tree__name">${escapeHtml(page.label)}</span>`;
      btn.onclick = () => {
        if (page.id === "runtime") go("runtime", state.page === "runtime" ? state.tab || "runners" : "runners");
        else if (page.id === "ops") go("ops", state.page === "ops" ? state.tab || "lifecycle" : "lifecycle");
        else go(page.id);
      };
      nav.appendChild(btn);
    }
  }

  async function renderPage() {
    const content = app.querySelector("#content");
    const title = app.querySelector("#page-title");
    const subtitle = app.querySelector("#page-subtitle");
    const pageMeta = PAGES.find((p) => p.id === state.page) || PAGES[0];
    title.textContent = pageMeta.label;
    subtitle.textContent = pageSubtitle(state.page, state.tab, { setupOnly });
    content.className = "page-body";
    content.innerHTML = "";
    state.error = "";
    state.busy = true;
    try {
      if (state.page === "overview") {
        await renderOverview(content, rpc, {
          go,
          includeConfig,
          includeQq,
          includeAgentConnections,
          includeBotFlow,
        });
      }
      else if (state.page === "runtime") await renderRuntime(content, rpc, app, state, go);
      else if (state.page === "upgrade") await renderUpgrade(content, rpc, app, state, go);
      else if (state.page === "plugins") await renderPlugins(content, rpc, app);
      else if (state.page === "tasks") await renderTasks(content, rpc, app, state);
      else if (state.page === "resources") await renderResources(content, rpc);
      else if (state.page === "database") await renderDatabase(content, rpc, app, builtinDatabases);
      else if (state.page === "config") await renderConfig(content, rpc);
      else if (state.page === "qq-bot") await renderQqBot(content, rpc);
      else if (state.page === "agent-connections") await renderAgentConnections(content, rpc);
      else if (state.page === "bot-flow") await renderBotFlow(content, rpc);
      else if (state.page === "bilibili") await renderBilibili(content, rpc);
      else if (state.page === "ops") await renderOps(content, rpc, app, state, go);
      else await renderOverview(content, rpc, { go });
    } catch (err) {
      state.error = formatRpcError(err);
      content.innerHTML = `<div class="error-banner"><strong>加载失败</strong><div class="muted">${escapeHtml(state.error)}</div></div>`;
    } finally {
      state.busy = false;
    }
  }

  app.innerHTML = `
    <aside class="lilia-workspace-region" data-region="navigation" data-region-separator="inline">
      <div class="secondary-panel">
        <div class="secondary-panel__top">
          <div class="brand">Mutsuki</div>
        </div>
        <nav class="secondary-panel__body sb-section nav" aria-label="控制台"></nav>
        <div class="secondary-panel__footer sidebar-footer">Bot 控制台</div>
      </div>
    </aside>
    <main class="lilia-workspace-region" data-region="main">
      <div class="lilia-workspace-region__content page-scroll">
        <div class="page-header">
          <div><h1 id="page-title">概览</h1><p id="page-subtitle"></p></div>
          <div class="page-actions"><button type="button" id="refresh" class="ghost">刷新</button></div>
        </div>
        <section id="content" class="page-body">加载中…</section>
      </div>
    </main>
  `;

  renderNav();
  app.querySelector("#refresh").onclick = renderPage;
  window.addEventListener("popstate", () => {
    const route = parseRoute();
    state.page = pageAvailable(route.page) ? route.page : (includeConfig ? "config" : "overview");
    state.tab = route.tab;
    renderNav();
    renderPage();
  });
  renderPage();
  return app;
}

function pageSubtitle(page, tab, options = {}) {
  switch (page) {
    case "upgrade":
      return "对照 release set 检查 Mutsuki 模块 Git pin，生成 fetch / build / ABI / pin 升级计划";
    case "plugins":
      return "插件清单与部署偏好";
    case "runtime":
      if (tab === "events") return "事件源连接与健康";
      if (tab === "topology") return "插件 → 运行器 → 事件源 关系图";
      return "运行器进程状态与运维操作";
    case "tasks":
      return "任务表、详情与事件时间线（调试提交默认折叠）";
    case "resources":
      return "Runtime ResourceRef 清单（只读）";
    case "database":
      return "产品内置 SQLite 只读浏览（经 task_submit_batch / mutsuki.db.*）";
    case "config":
      return options.setupOnly ? "启用本机 Bot 工作区" : "管理 QQ 账号、模型和回复策略";
    case "qq-bot":
      return "QQ 登录、连接状态与投递";
    case "agent-connections":
      return "查看助手状态、会话与用量";
    case "bot-flow":
      return "编排 QQ 消息、Agent 与回复步骤并发布";
    case "bilibili":
      return "B 站推送登陆态、扫码登录与订阅管理";
    case "ops":
      return tab === "logs" ? "运行时日志尾部" : "Core drain 与 Service 关闭（强确认 + runtime.write）";
    default:
      return "QQ Bot 与智能助手状态";
  }
}

async function renderOverview(content, rpc, ctx = {}) {
  content.className = "page-body overview-dashboard";
  const go = ctx.go || (() => {});
  const d = await rpc.read("overview", "summary");
  const h = d.health || {};
  const workspaceAvailable = ctx.includeQq || ctx.includeAgentConnections || ctx.includeBotFlow;

  const metrics = [
    { label: "运行时间", value: formatDuration(d.uptime_ms) },
    { label: workspaceAvailable ? "Bot 服务" : "管理后台", value: healthLabel(h.service) },
  ];
  if (workspaceAvailable) {
    metrics.push({ label: "消息处理", value: healthLabel(h.core) });
  }
  appendMetricGrid(content, metrics);

  const grid = document.createElement("div");
  grid.className = "overview-grid";
  content.appendChild(grid);

  const entry = document.createElement("section");
  entry.className = "card";
  entry.innerHTML = `<h2>开始使用</h2><p class="muted">配置可用的账号和助手，再发布消息处理流程。</p>`;
  const actions = document.createElement("div");
  actions.className = "toolbar nested";
  const availableActions = [];
  if (ctx.includeConfig) availableActions.push(["配置账号与助手", "config"]);
  if (ctx.includeBotFlow) availableActions.push(["编排回复流程", "bot-flow"]);
  if (ctx.includeQq) availableActions.push(["QQ 登录与状态", "qq-bot"]);
  if (ctx.includeAgentConnections) availableActions.push(["查看 Agent 会话", "agent-connections"]);
  for (const [label, page] of availableActions) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "ghost";
    button.textContent = label;
    button.onclick = () => go(page);
    actions.appendChild(button);
  }
  entry.appendChild(actions);
  grid.appendChild(entry);
}

function appendMetricGrid(content, metrics) {
  const grid = document.createElement("div");
  grid.className = "metric-grid";
  for (const metric of metrics) {
    const card = document.createElement(metric.onClick ? "button" : "div");
    if (metric.onClick) {
      card.type = "button";
      card.className = "metric-card metric-card--link";
      card.onclick = metric.onClick;
    } else {
      card.className = "metric-card";
    }
    card.innerHTML = `<div class="metric-label">${escapeHtml(metric.label)}</div><div class="metric-value">${escapeHtml(metric.value)}</div>`;
    grid.appendChild(card);
  }
  content.appendChild(grid);
}

/** @param {[string, unknown, boolean?][]} rows */
function appendKvCard(content, title, rows) {
  const items = rows
    .map(([label, value, asHealth]) => {
      const display = asHealth
        ? healthLabel(value)
        : value == null || value === ""
          ? "—"
          : String(value);
      const text = escapeHtml(display);
      const cls = asHealth ? healthClass(value) || "muted" : null;
      const right = cls ? `<span class="status-${cls}">${text}</span>` : `<span>${text}</span>`;
      return `<li><span>${escapeHtml(label)}</span>${right}</li>`;
    })
    .join("");
  return appendSection(content, title, `<ul class="kv">${items}</ul>`);
}

async function renderSecretStatusSection(content, rpc) {
  try {
    const body = await rpc.read("secret", "status");
    const secrets = body?.secrets || [];
    if (!secrets.length) return;
    appendSection(content, "密钥状态", renderSecretRows(secrets));
  } catch {
    // Secret monitor not configured for this console build.
  }
}

function renderSecretRows(secrets) {
  return `<ul class="kv">${secrets
    .map((item) => {
      const state = String(item.state || "absent");
      const label = secretStateLabel(state);
      const cls = secretStateClass(state);
      return `<li><span>${escapeHtml(item.key)}</span><span class="status-${cls}">${label}</span></li>`;
    })
    .join("")}</ul>`;
}

function secretStateLabel(state) {
  switch (state) {
    case "present":
      return "已配置";
    case "invalid":
      return "无效";
    default:
      return "缺失";
  }
}

function secretStateClass(state) {
  switch (state) {
    case "present":
      return "ok";
    case "invalid":
      return "err";
    default:
      return "warn";
  }
}

async function renderPlugins(content, rpc, app) {
  const toolbar = document.createElement("div");
  toolbar.className = "toolbar";
  toolbar.innerHTML = `<button type="button" class="ghost" id="reload-plugins">重载插件</button>`;
  content.appendChild(toolbar);

  const plugins = await rpc.read("control", "plugin_list");
  const list = plugins?.plugins || [];
  const diagnostics = plugins?.diagnostics || [];

  if (diagnostics.length) {
    appendSection(content, "清单诊断", renderPluginDiagnostics(diagnostics));
  }

  if (!list.length) {
    content.appendChild(emptyBlock("暂无插件"));
  } else {
    for (const p of list) {
      content.appendChild(renderPluginCard(p, rpc, app));
    }
  }

  toolbar.querySelector("#reload-plugins").onclick = async () => {
    if (!confirmAction("确认重载全部插件？Runners 将按 Host 策略重启。")) return;
    try {
      await rpc.write("control", "plugin_reload");
      flash(app, "插件重载已提交");
      await renderPlugins(content, rpc, app);
    } catch (err) {
      flash(app, formatRpcError(err), true);
    }
  };
}

function renderPluginDiagnostics(diagnostics) {
  return diagnostics
    .map((d) => {
      const id = d.plugin_id ? `${escapeHtml(d.plugin_id)} · ` : "";
      const deployment = d.deployment ? `${escapeHtml(d.deployment)} · ` : "";
      return `<div class="tree-item"><strong>${escapeHtml(d.manifest_path || "manifest")}</strong><div class="muted">${id}${deployment}${escapeHtml(d.detail || "—")}</div></div>`;
    })
    .join("");
}

function renderPluginCard(plugin, rpc, app) {
  const el = document.createElement("div");
  el.className = "plugin-card";
  const header = document.createElement("div");
  header.className = "tree-item";
  header.innerHTML = `
    <strong>${escapeHtml(plugin.plugin_id)}</strong>
    <div class="muted">active=${escapeHtml(plugin.active_deployment || "—")} · preferred=${escapeHtml(plugin.preferred_deployment || "—")} · configured=${plugin.configured ? "yes" : "no"}</div>
  `;
  el.appendChild(header);

  const candidates = plugin.candidates || [];
  if (candidates.length) {
    const section = document.createElement("div");
    section.className = "section nested";
    section.innerHTML = "<h3>候选部署</h3>";
    for (const c of candidates) {
      section.appendChild(renderCandidateRow(plugin.plugin_id, c, rpc, app));
    }
    el.appendChild(section);
  }

  if (plugin.preferred_deployment) {
    const actions = document.createElement("div");
    actions.className = "toolbar nested";
    const clearBtn = document.createElement("button");
    clearBtn.type = "button";
    clearBtn.className = "ghost";
    clearBtn.textContent = "清除部署偏好";
    clearBtn.onclick = async () => {
      if (!confirmAction(`清除 ${plugin.plugin_id} 的部署偏好？`)) return;
      try {
        await rpc.write("control", "plugin_deployment_clear", { plugin_id: plugin.plugin_id });
        flash(app, "部署偏好已清除");
        await rerenderPlugins(app, rpc);
      } catch (err) {
        flash(app, formatRpcError(err), true);
      }
    };
    actions.appendChild(clearBtn);
    el.appendChild(actions);
  }

  return el;
}

function renderCandidateRow(pluginId, candidate, rpc, app) {
  const row = document.createElement("div");
  row.className = "tree-item row-item candidate-row";
  const linkNote = candidate.runner_link
    ? ` · link=${candidate.runner_link}`
    : " · link=—";
  row.innerHTML = `
    <div>
      <strong>${escapeHtml(candidate.deployment)}</strong>
      <div class="muted">${escapeHtml(candidate.version)} · api=${escapeHtml(candidate.api_version)} · ${candidate.available ? "可用" : "不可用"}${escapeHtml(linkNote)}</div>
      <div class="muted mono">${escapeHtml(candidate.sha256?.slice(0, 12) || "—")}… · ${escapeHtml(candidate.path || "—")}</div>
    </div>
  `;
  const actions = document.createElement("div");
  actions.className = "row-actions";
  const setBtn = document.createElement("button");
  setBtn.type = "button";
  setBtn.className = "ghost";
  setBtn.textContent = "设为偏好";
  setBtn.disabled = !candidate.available;
  setBtn.onclick = async () => {
    if (
      !confirmAction(
        `将 ${pluginId} 的部署偏好设为 ${candidate.deployment}？`,
      )
    )
      return;
    try {
      await rpc.write("control", "plugin_deployment_set", {
        plugin_id: pluginId,
        deployment: candidate.deployment,
      });
      flash(app, `已设置 ${pluginId} → ${candidate.deployment}`);
      await rerenderPlugins(app, rpc);
    } catch (err) {
      flash(app, formatRpcError(err), true);
    }
  };
  actions.appendChild(setBtn);
  row.appendChild(actions);
  return row;
}

async function rerenderPlugins(app, rpc) {
  const content = app.querySelector("#content");
  content.innerHTML = "";
  await renderPlugins(content, rpc, app);
}

function renderTabBar(tabs, active, onSelect) {
  const bar = document.createElement("div");
  bar.className = "tab-bar";
  bar.setAttribute("role", "tablist");
  for (const tab of tabs) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = `tab-bar__item${tab.id === active ? " is-active" : ""}`;
    btn.setAttribute("role", "tab");
    btn.setAttribute("aria-selected", tab.id === active ? "true" : "false");
    btn.textContent = tab.label;
    btn.onclick = () => onSelect(tab.id);
    bar.appendChild(btn);
  }
  return bar;
}

async function renderRuntime(content, rpc, app, state, go) {
  content.appendChild(
    renderTabBar(RUNTIME_TABS, state.tab || "runners", (tab) => go("runtime", tab)),
  );
  const body = document.createElement("div");
  body.className = "tab-panel";
  content.appendChild(body);
  if (state.tab === "events") await renderEvents(body, rpc, app);
  else if (state.tab === "topology") await renderTopology(body, rpc, go);
  else await renderRunners(body, rpc, app);
}

async function renderOps(content, rpc, app, state, go) {
  content.appendChild(
    renderTabBar(OPS_TABS, state.tab || "lifecycle", (tab) => go("ops", tab)),
  );
  const body = document.createElement("div");
  body.className = "tab-panel";
  content.appendChild(body);
  if (state.tab === "logs") await renderLogs(body, rpc);
  else await renderLifecycle(body, rpc, app);
}

const panelModules = new Map();
const panelModuleAttempts = new Map();

async function loadPanelModule(panelId, specifier) {
  const cached = panelModules.get(panelId);
  if (cached) return cached;

  const attempt = (panelModuleAttempts.get(panelId) || 0) + 1;
  panelModuleAttempts.set(panelId, attempt);
  const separator = specifier.includes("?") ? "&" : "?";
  const request = import(`${specifier}${separator}attempt=${attempt}`).catch((error) => {
    if (panelModules.get(panelId) === request) panelModules.delete(panelId);
    throw error;
  });
  panelModules.set(panelId, request);
  return request;
}

async function renderConfig(content, rpc) {
  try {
    const mod = await loadPanelModule("config", "./config/index.js");
    if (typeof mod.mountConfigPanel !== "function") {
      content.appendChild(emptyBlock("配置页面暂时不可用，请刷新后重试。"));
      return;
    }
    mod.mountConfigPanel(content, rpc);
  } catch (err) {
    console.error("Config panel failed to load", err);
    content.innerHTML = `<div class="error-banner"><strong>配置页不可用</strong><div class="muted">请刷新后重试。</div></div>`;
  }
}

async function renderBilibili(content, rpc) {
  try {
    const mod = await loadPanelModule("bilibili", "./bilibili/index.js");
    if (typeof mod.mountBilibiliPanel !== "function") {
      content.appendChild(
        emptyBlock("B站推送扩展未提供 mountBilibiliPanel；请确认 bilibili 插件已装配。"),
      );
      return;
    }
    mod.mountBilibiliPanel(content, rpc);
  } catch (err) {
    console.error("Bilibili panel failed to load", err);
    content.innerHTML = `<div class="error-banner"><strong>B站推送页不可用</strong><div class="muted">请刷新后重试。</div></div>`;
  }
}

async function renderQqBot(content, rpc) {
  try {
    const mod = await loadPanelModule("qq-bot", "./qq-bot/index.js");
    if (typeof mod.mountQqBotPanel !== "function") {
      content.appendChild(emptyBlock("QQ 管理页面暂时不可用，请刷新后重试。"));
      return;
    }
    mod.mountQqBotPanel(content, rpc);
  } catch (err) {
    console.error("QQ management panel failed to load", err);
    content.innerHTML = `<div class="error-banner"><strong>QQ 管理页不可用</strong><div class="muted">请刷新后重试。</div></div>`;
  }
}

async function renderAgentConnections(content, rpc) {
  try {
    const mod = await loadPanelModule("bot-agent", "./bot-agent/index.js");
    if (typeof mod.mountAgentConnectionsPanel !== "function") {
      content.appendChild(emptyBlock("Agent 管理页面暂时不可用，请刷新后重试。"));
      return;
    }
    await mod.mountAgentConnectionsPanel(content, rpc);
  } catch (err) {
    console.error("Agent management panel failed to load", err);
    content.innerHTML = `<div class="error-banner"><strong>Agent 管理页不可用</strong><div class="muted">请刷新后重试。</div></div>`;
  }
}

async function renderBotFlow(content, rpc) {
  try {
    const mod = await loadPanelModule("bot-flow", "./bot-flow/index.js");
    if (typeof mod.mountBotFlowEditor !== "function") {
      content.appendChild(emptyBlock("流程编排页面暂时不可用，请刷新后重试。"));
      return;
    }
    await mod.mountBotFlowEditor(content, rpc);
  } catch (err) {
    console.error("Bot Flow panel failed to load", err);
    content.innerHTML = `<div class="error-banner"><strong>流程编排页不可用</strong><div class="muted">请刷新后重试。</div></div>`;
  }
}

async function renderTopology(content, rpc, go) {
  const [pluginsBody, runners, sources] = await Promise.all([
    rpc.read("control", "plugin_list"),
    rpc.read("control", "runner_list"),
    rpc.read("control", "event_source_list"),
  ]);
  const plugins = pluginsBody?.plugins || [];
  const runnerList = runners || [];
  const sourceList = sources || [];

  if (!plugins.length && !runnerList.length && !sourceList.length) {
    content.appendChild(emptyBlock("暂无拓扑数据"));
    return;
  }

  const byPlugin = new Map();
  for (const plugin of plugins) {
    byPlugin.set(plugin.plugin_id, {
      plugin,
      runners: [],
      sources: [],
    });
  }
  for (const runner of runnerList) {
    const bucket = byPlugin.get(runner.plugin_id) || {
      plugin: { plugin_id: runner.plugin_id },
      runners: [],
      sources: [],
    };
    bucket.runners.push(runner);
    byPlugin.set(runner.plugin_id, bucket);
  }
  for (const source of sourceList) {
    const bucket = byPlugin.get(source.plugin_id) || {
      plugin: { plugin_id: source.plugin_id },
      runners: [],
      sources: [],
    };
    bucket.sources.push(source);
    byPlugin.set(source.plugin_id, bucket);
  }

  const graph = document.createElement("div");
  graph.className = "topology-graph";
  for (const [pluginId, node] of byPlugin) {
    const card = document.createElement("section");
    card.className = "topology-node card";
    const head = document.createElement("button");
    head.type = "button";
    head.className = "topology-node__plugin ghost";
    head.innerHTML = `<strong>${escapeHtml(pluginId)}</strong><span class="muted">插件</span>`;
    head.onclick = () => go("plugins");
    card.appendChild(head);

    const lanes = document.createElement("div");
    lanes.className = "topology-node__lanes";

    const runnerLane = document.createElement("div");
    runnerLane.className = "topology-lane";
    runnerLane.innerHTML = "<h3>运行器</h3>";
    if (!node.runners.length) {
      runnerLane.appendChild(emptyBlock("无"));
    } else {
      for (const runner of node.runners) {
        const btn = document.createElement("button");
        btn.type = "button";
        btn.className = "topology-chip";
        btn.innerHTML = `<strong>${escapeHtml(runner.runner_id)}</strong><span class="muted">${escapeHtml(runner.state)}</span>`;
        btn.onclick = () => go("runtime", "runners");
        runnerLane.appendChild(btn);
      }
    }
    lanes.appendChild(runnerLane);

    const sourceLane = document.createElement("div");
    sourceLane.className = "topology-lane";
    sourceLane.innerHTML = "<h3>事件源</h3>";
    if (!node.sources.length) {
      sourceLane.appendChild(emptyBlock("无"));
    } else {
      for (const source of node.sources) {
        const btn = document.createElement("button");
        btn.type = "button";
        btn.className = "topology-chip";
        btn.innerHTML = `<strong>${escapeHtml(source.source_id)}</strong><span class="muted">${escapeHtml(source.health || source.state)}</span>`;
        btn.onclick = () => go("runtime", "events");
        sourceLane.appendChild(btn);
      }
    }
    lanes.appendChild(sourceLane);
    card.appendChild(lanes);
    graph.appendChild(card);
  }
  content.appendChild(graph);
}

async function renderResources(content, rpc) {
  appendSection(
    content,
    "ResourceRef 资源",
    `<div class="muted capability-gap">Runtime 资源清单控制面尚未暴露（Core 已补 <code>ResourceManager::list_descriptors</code>，待 Host <code>resource_list</code> pin）。此处不展示假列表。</div>`,
  );
  try {
    const body = await rpc.read("control", "resource_list");
    const resources = body?.resources || body || [];
    if (!Array.isArray(resources) || !resources.length) {
      content.appendChild(emptyBlock("暂无已注册 ResourceRef"));
      return;
    }
    for (const item of resources) {
      const el = document.createElement("div");
      el.className = "tree-item";
      el.innerHTML = `<strong>${escapeHtml(item.ref_id || "—")}</strong><div class="muted">${escapeHtml(item.provider_id || "—")} · ${escapeHtml(item.resource_kind || "—")} · v${escapeHtml(String(item.version ?? "—"))}</div>`;
      content.appendChild(el);
    }
  } catch (err) {
    const note = document.createElement("div");
    note.className = "muted";
    note.textContent = `resource_list 不可用：${formatRpcError(err)}`;
    content.appendChild(note);
  }
}

async function renderDatabase(content, rpc, app, builtinDatabases) {
  const databases = (builtinDatabases || []).filter((item) => item && item.path);
  if (!databases.length) {
    appendSection(
      content,
      "内置数据库",
      `<div class="muted capability-gap">产品未声明内置 SQLite 路径（console-options.builtinDatabases）。禁止任意路径浏览；请在产品装配中提供 allowlist。</div>`,
    );
    return;
  }

  const toolbar = document.createElement("div");
  toolbar.className = "toolbar";
  const select = document.createElement("select");
  select.id = "db-select";
  for (const db of databases) {
    const opt = document.createElement("option");
    opt.value = db.path;
    opt.textContent = db.label || db.path;
    select.appendChild(opt);
  }
  const refreshBtn = document.createElement("button");
  refreshBtn.type = "button";
  refreshBtn.className = "ghost";
  refreshBtn.textContent = "列出表";
  toolbar.append(select, refreshBtn);
  content.appendChild(toolbar);

  const output = document.createElement("div");
  content.appendChild(output);

  async function submitDbQuery(path, sql) {
    const allowlist = databases.map((item) => item.path);
    const batch = {
      batch: {
        batch_id: `console-db-${Date.now()}`,
        tasks: [
          {
            task_id: `db-query-${Date.now()}`,
            protocol_id: "mutsuki.db.query",
            input: {
              path,
              sql,
              readonly: true,
              db_path_allowlist: allowlist,
            },
          },
        ],
      },
    };
    return rpc.write("control", "task_submit_batch", batch);
  }

  refreshBtn.onclick = async () => {
    output.innerHTML = "<div class='muted'>查询中…</div>";
    try {
      const path = select.value;
      const result = await submitDbQuery(
        path,
        "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name",
      );
      output.innerHTML = `<div class="card"><h2>表清单</h2><pre class="log-block">${escapeHtml(JSON.stringify(result, null, 2))}</pre><p class="muted">只读；写入操作默认关闭。结果为 task_submit_batch 句柄，完整行需 Core 运行并等待 outcome。</p></div>`;
      flash(app, "已提交只读表清单查询");
    } catch (err) {
      output.innerHTML = `<div class="err-text">${escapeHtml(formatRpcError(err))}</div>`;
      flash(app, formatRpcError(err), true);
    }
  };
}

async function renderRunners(content, rpc, app) {
  const runners = await rpc.read("control", "runner_list");
  if (!runners?.length) {
    content.appendChild(emptyBlock("暂无 Runner"));
    return;
  }
  for (const r of runners) {
    const el = document.createElement("div");
    el.className = "tree-item row-item";
    el.innerHTML = `
      <div>
        <strong>${r.runner_id}</strong>
        <div class="muted">${r.plugin_id} · ${r.state} · pid=${r.pid ?? "—"} · restarts=${r.restarts ?? 0}</div>
      </div>
      <div class="row-actions">
        <button type="button" class="ghost" data-action="restart" data-id="${r.runner_id}">重启</button>
        <button type="button" class="ghost danger" data-action="stop" data-id="${r.runner_id}">停止</button>
      </div>
    `;
    content.appendChild(el);
  }
  content.querySelectorAll("[data-action]").forEach((btn) => {
    btn.onclick = async () => {
      const id = btn.getAttribute("data-id");
      const action = btn.getAttribute("data-action");
      const label = action === "restart" ? "重启" : "停止";
      if (!confirmAction(`确认${label} Runner ${id}？`)) return;
      try {
        if (action === "restart") await rpc.write("control", "runner_restart", { id });
        else await rpc.write("control", "runner_stop", { id });
        flash(app, `${label} ${id} 已提交`);
      } catch (err) {
        flash(app, formatRpcError(err), true);
      }
    };
  });
}

async function renderEvents(content, rpc, app) {
  const sources = await rpc.read("control", "event_source_list");
  if (!sources?.length) {
    content.appendChild(emptyBlock("暂无 EventSource"));
    return;
  }
  for (const s of sources) {
    const el = document.createElement("div");
    el.className = "tree-item row-item";
    const errLine = s.last_error
      ? `<div class="muted err-text">${escapeHtml(s.last_error)}</div>`
      : "";
    el.innerHTML = `
      <div>
        <strong>${escapeHtml(s.source_id)}</strong>
        <div class="muted">${escapeHtml(s.plugin_id)} · ${escapeHtml(s.state)}/${escapeHtml(s.health)} · reconnects=${s.reconnects ?? 0}</div>
        ${errLine}
      </div>
      <div class="row-actions">
        <button type="button" class="ghost" data-restart="${escapeHtml(s.source_id)}">重启</button>
      </div>
    `;
    content.appendChild(el);
  }
  content.querySelectorAll("[data-restart]").forEach((btn) => {
    btn.onclick = async () => {
      const id = btn.getAttribute("data-restart");
      if (!confirmAction(`确认重启 EventSource ${id}？`)) return;
      try {
        await rpc.write("control", "event_source_restart", { id });
        flash(app, `EventSource ${id} 重启已提交`);
        content.innerHTML = "";
        await renderEvents(content, rpc, app);
      } catch (err) {
        flash(app, formatRpcError(err), true);
      }
    };
  });
}

async function renderLogs(content, rpc) {
  const logs = await rpc.read("control", "log_tail", { lines: 50 });
  appendSection(content, "日志尾部", renderLogLines(logs?.entries || []));
}

function renderLogLines(entries) {
  if (!entries.length) return "<div class='muted'>暂无日志</div>";
  return `<pre class="log-block">${entries.map((e) => escapeHtml(e.line)).join("\n")}</pre>`;
}

function renderTaskRows(tasks) {
  if (!tasks.length) return "<div class='muted'>暂无任务</div>";
  return tasks
    .map(
      (t) =>
        `<div class="tree-item row-item"><div><strong>${escapeHtml(t.task_id)}</strong><div class="muted">${escapeHtml(t.protocol_id)} · ${escapeHtml(t.status)} · hint=${escapeHtml(t.runner_hint || "—")}</div></div><div class="row-actions"><button type="button" class="ghost" data-cancel-task="${escapeHtml(t.task_id)}">取消</button></div></div>`,
    )
    .join("");
}

async function renderTasks(content, rpc, app, state) {
  const toolbar = document.createElement("div");
  toolbar.className = "toolbar row-item";
  toolbar.innerHTML = `
    <button type="button" class="ghost" id="tasks-refresh">刷新列表</button>
    <span class="muted">详情与事件时间线 · 调试提交默认折叠</span>
  `;
  content.appendChild(toolbar);

  const layout = document.createElement("div");
  layout.className = "tasks-layout";
  content.appendChild(layout);

  const tableCard = document.createElement("section");
  tableCard.className = "card";
  tableCard.innerHTML = "<h2>任务表</h2>";
  const tableHost = document.createElement("div");
  tableCard.appendChild(tableHost);
  layout.appendChild(tableCard);

  const detailCard = document.createElement("section");
  detailCard.className = "card";
  detailCard.innerHTML = "<h2>任务详情</h2><div id='task-detail' class='muted'>选择左侧任务</div>";
  layout.appendChild(detailCard);

  const timelineCard = document.createElement("section");
  timelineCard.className = "card";
  timelineCard.innerHTML = `
    <h2>事件时间线</h2>
    <p class="muted">按任务聚焦当前窗口中的 RuntimeEvent。无时间戳时按序号投影。</p>
    <div id="task-trajectory" class="trajectory"></div>
  `;
  content.appendChild(timelineCard);
  const trajHost = timelineCard.querySelector("#task-trajectory");
  let traj = null;
  let loadedEvents = [];
  let earliestAvailable = 0;
  let pageLimit = 100;

  const advanced = document.createElement("details");
  advanced.className = "card advanced-fold";
  advanced.innerHTML = `
    <summary>高级 / 调试 · submit_batch</summary>
    <p class="muted">提交合法 TaskBatch JSON；空 batch 会被 ServiceHost 拒绝。需要 runtime.write。</p>
    <textarea id="task-submit-json" class="log-block" rows="8">${escapeHtml(DEFAULT_TASK_BATCH_JSON)}</textarea>
    <div class="toolbar nested">
      <button type="button" class="ghost" id="task-submit-btn">提交 batch</button>
    </div>
    <div id="task-submit-output" class="muted"></div>
  `;
  content.appendChild(advanced);

  function renderDetail(task) {
    const host = detailCard.querySelector("#task-detail");
    if (!task) {
      host.className = "muted";
      host.textContent = "选择左侧任务";
      return;
    }
    host.className = "";
    host.innerHTML = `<ul class="kv">
      <li><span>task_id</span><span class="mono">${escapeHtml(task.task_id)}</span></li>
      <li><span>protocol</span><span>${escapeHtml(task.protocol_id)}</span></li>
      <li><span>status</span><span>${escapeHtml(task.status)}</span></li>
      <li><span>runner_hint</span><span>${escapeHtml(task.runner_hint || "—")}</span></li>
      <li><span>owner_runner</span><span>${escapeHtml(task.owner_runner || "—")}</span></li>
      <li><span>correlation</span><span>${escapeHtml(task.correlation_id || "—")}</span></li>
      <li><span>trace</span><span>${escapeHtml(task.trace_id || "—")}</span></li>
    </ul>
    <div class="toolbar nested">
      <button type="button" class="ghost" data-cancel-selected>取消任务</button>
    </div>`;
    host.querySelector("[data-cancel-selected]").onclick = async () => {
      if (!confirmAction(`确认取消任务 ${task.task_id}？`)) return;
      try {
        await rpc.write("control", "task_cancel", { id: task.task_id });
        flash(app, `任务 ${task.task_id} 取消已提交`);
        await loadTasks();
      } catch (err) {
        flash(app, formatRpcError(err), true);
      }
    };
  }

  async function loadTasks() {
    tableHost.innerHTML = "<div class='muted'>加载任务…</div>";
    const tasks = (await rpc.read("control", "task_list")) || [];
    if (!tasks.length) {
      tableHost.innerHTML = "<div class='muted'>暂无任务</div>";
      renderDetail(null);
      await loadTaskEvents({ reset: true });
      return;
    }
    const table = document.createElement("table");
    table.className = "data-table";
    table.innerHTML = `<thead><tr><th>任务</th><th>协议</th><th>状态</th><th>运行器</th></tr></thead>`;
    const tbody = document.createElement("tbody");
    for (const task of tasks) {
      const tr = document.createElement("tr");
      if (state.selectedTaskId === task.task_id) tr.classList.add("is-selected");
      tr.innerHTML = `<td class="mono">${escapeHtml(task.task_id)}</td><td>${escapeHtml(task.protocol_id)}</td><td>${escapeHtml(task.status)}</td><td>${escapeHtml(task.runner_hint || "—")}</td>`;
      tr.onclick = () => {
        state.selectedTaskId = task.task_id;
        loadTasks();
      };
      tbody.appendChild(tr);
    }
    table.appendChild(tbody);
    tableHost.innerHTML = "";
    tableHost.appendChild(table);
    const selected = tasks.find((item) => item.task_id === state.selectedTaskId) || tasks[0];
    state.selectedTaskId = selected.task_id;
    renderDetail(selected);
    await loadTaskEvents({ reset: true });
  }

  toolbar.querySelector("#tasks-refresh").onclick = () =>
    loadTasks().catch((err) => flash(app, formatRpcError(err), true));

  async function ensureTrajectory() {
    if (traj) return traj;
    const [
      { mountTrajectoryView },
      { projectRuntimeEvents, mergeBySequence, sequenceOf, trimWindow, EVENT_WINDOW_LIMIT },
    ] = await Promise.all([
      import("./trajectory-view.js"),
      import("./trajectory-model.js"),
    ]);
    function project() {
      return projectRuntimeEvents(loadedEvents, { focusSubjectId: state.selectedTaskId });
    }
    function firstSequence() {
      const values = loadedEvents.map(sequenceOf).filter((value) => value != null);
      return values.length ? Math.min(...values) : earliestAvailable + 1;
    }
    traj = mountTrajectoryView(trajHost, {
      ...project(),
      hasOlder: false,
      emptyText: "当前窗口没有该任务的事件",
      onInspect: async (record) =>
        loadedEvents.filter((event) => (record.sequences || []).includes(sequenceOf(event))),
      onLoadOlder: async () => {
        await loadTaskEvents({ reset: false });
      },
    });
    traj._project = project;
    traj._mergeBySequence = mergeBySequence;
    traj._sequenceOf = sequenceOf;
    traj._firstSequence = firstSequence;
    traj._trimWindow = trimWindow;
    traj._windowLimit = EVENT_WINDOW_LIMIT;
    return traj;
  }

  async function loadTaskEvents({ reset }) {
    const view = await ensureTrajectory();
    try {
      if (reset) {
        loadedEvents = [];
        view.update({ loading: true, records: [], groups: [], emptyText: "正在加载时间线…" });
      } else {
        view.update({ loadingOlder: true });
      }
      const probe = reset
        ? await rpc.read("control", "task_events_after", { sequence: 0, limit: 1 })
        : null;
      if (probe) {
        earliestAvailable = Number(probe.earliest_available_sequence || 0);
        const latest = Number(probe.latest_sequence || 0);
        pageLimit = 100;
        const after = Math.max(
          Math.max(0, earliestAvailable ? earliestAvailable - 1 : 0),
          latest > pageLimit ? latest - pageLimit : Math.max(0, earliestAvailable ? earliestAvailable - 1 : 0),
        );
        const page = await rpc.read("control", "task_events_after", {
          sequence: after,
          limit: pageLimit,
        });
        loadedEvents = page?.events || page?.items || [];
        view.update({
          ...view._project(),
          loading: false,
          loadingOlder: false,
          hasOlder: view._firstSequence() > (earliestAvailable || 1),
          lost: Number(page?.lost || probe.lost || 0),
          dropped: Number(page?.dropped || probe.dropped || 0),
          truncated: !!(page?.has_more || page?.truncated),
          emptyText: "当前窗口没有该任务的事件",
        });
        return;
      }
      const first = view._firstSequence();
      if (first <= (earliestAvailable || 1)) {
        view.update({ loadingOlder: false, hasOlder: false });
        return;
      }
      const after = Math.max(
        Math.max(0, earliestAvailable ? earliestAvailable - 1 : 0),
        first - pageLimit - 1,
      );
      const page = await rpc.read("control", "task_events_after", {
        sequence: after,
        limit: pageLimit,
      });
      const older = (page?.events || page?.items || []).filter((event) => {
        const seq = view._sequenceOf(event);
        return seq != null && seq < first;
      });
      loadedEvents = view._trimWindow(
        view._mergeBySequence(loadedEvents, older),
        view._windowLimit,
        "older",
      );
      view.update({
        ...view._project(),
        loadingOlder: false,
        hasOlder: view._firstSequence() > (earliestAvailable || 1),
        lost: Number(page?.lost || 0),
        dropped: Number(page?.dropped || 0),
        truncated: !!(page?.has_more || page?.truncated),
      });
    } catch (err) {
      view.update({
        loading: false,
        loadingOlder: false,
        emptyText: formatRpcError(err),
      });
      flash(app, formatRpcError(err), true);
    }
  }

  advanced.querySelector("#task-submit-btn").onclick = async () => {
    const raw = advanced.querySelector("#task-submit-json").value;
    const output = advanced.querySelector("#task-submit-output");
    if (!confirmAction("确认提交 TaskBatch？此操作会进入 Core 调度。")) return;
    try {
      const payload = JSON.parse(raw);
      const result = await rpc.write("control", "task_submit_batch", payload);
      output.innerHTML = `<pre class="log-block">${escapeHtml(JSON.stringify(result, null, 2))}</pre>`;
      flash(app, "TaskBatch 已提交");
      await loadTasks();
    } catch (err) {
      output.innerHTML = `<div class="err-text">${escapeHtml(formatRpcError(err))}</div>`;
      flash(app, formatRpcError(err), true);
    }
  };

  await loadTasks();
}

const DEFAULT_TASK_BATCH_JSON = `{
  "batch": {
    "batch_id": "console-debug",
    "tasks": [
      {
        "task_id": "debug-task-1",
        "protocol_id": "control.input",
        "input": { "value": 1 }
      }
    ]
  }
}`;

async function renderLifecycle(content, rpc, app) {
  content.innerHTML = `
    <div class="card">
      <h2>Core drain</h2>
      <p class="muted">停止接受新 Task 并进入 draining。需要 runtime.write 与二次确认。</p>
      <button type="button" class="ghost" id="core-drain-btn">开始 Core drain</button>
      <div id="core-drain-output" class="muted"></div>
    </div>
    <div class="card">
      <h2>Service shutdown</h2>
      <p class="muted">触发 ServiceHost 优雅关闭。需要 runtime.write 与输入 SHUTDOWN 确认。</p>
      <button type="button" class="ghost danger" id="service-shutdown-btn">关闭 Service</button>
      <div id="service-shutdown-output" class="muted"></div>
    </div>
  `;

  content.querySelector("#core-drain-btn").onclick = async () => {
    if (!confirmDestructiveAction("Core drain", "DRAIN")) return;
    const output = content.querySelector("#core-drain-output");
    output.textContent = "提交中…";
    try {
      const result = await rpc.write("control", "core_begin_drain");
      output.innerHTML = `<pre class="log-block">${escapeHtml(JSON.stringify(result, null, 2))}</pre>`;
      flash(app, "Core drain 已提交");
    } catch (err) {
      output.innerHTML = `<div class="err-text">${escapeHtml(formatRpcError(err))}</div>`;
      flash(app, formatRpcError(err), true);
    }
  };

  content.querySelector("#service-shutdown-btn").onclick = async () => {
    if (!confirmDestructiveAction("Service 关闭", "SHUTDOWN")) return;
    const output = content.querySelector("#service-shutdown-output");
    output.textContent = "提交中…";
    try {
      await rpc.write("control", "service_shutdown");
      output.textContent = "关闭信号已发送";
      flash(app, "Service shutdown 已提交");
    } catch (err) {
      output.innerHTML = `<div class="err-text">${escapeHtml(formatRpcError(err))}</div>`;
      flash(app, formatRpcError(err), true);
    }
  };
}

function renderComponents(comps) {
  const ids = Object.keys(comps);
  if (!ids.length) return "<div class='muted'>暂无</div>";
  return `<ul class="kv">${ids
    .map((id) => {
      const snap = comps[id] || {};
      const started = snap.started_at_unix_ms ?? snap.connected_since_unix_ms;
      const status = snap.status ?? "—";
      const cls = healthClass(status);
      return `<li><span>${escapeHtml(componentLabel(id))}</span><span class="status-${cls || "muted"}">${escapeHtml(healthLabel(status))} · ${formatDuration(elapsed(started))}</span></li>`;
    })
    .join("")}</ul>`;
}

function appendSection(content, title, html) {
  const el = document.createElement("section");
  el.className = "card";
  el.innerHTML = `<h2>${title}</h2>${html}`;
  content.appendChild(el);
  return el;
}

function emptyBlock(text) {
  const el = document.createElement("div");
  el.className = "muted";
  el.textContent = text;
  return el;
}

function confirmAction(message) {
  return globalThis.confirm?.(message) !== false;
}

function confirmDestructiveAction(label, token) {
  if (!confirmAction(`即将执行 ${label}。此操作会影响正在运行的服务，是否继续？`)) {
    return false;
  }
  const typed = globalThis.prompt?.(`请输入 ${token} 以确认 ${label}`) ?? "";
  return typed.trim() === token;
}

function flash(app, message, isError = false) {
  let banner = app.querySelector(".flash-banner");
  if (!banner) {
    banner = document.createElement("div");
    banner.className = "flash-banner";
    const host = app.querySelector(".page-scroll") || app.querySelector("#content");
    host?.prepend(banner);
  }
  banner.className = `flash-banner${isError ? " error" : ""}`;
  banner.textContent = message;
  setTimeout(() => banner.remove(), 3500);
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;");
}

export async function loadConsoleOptions() {
  try {
    const response = await fetch("./console-options.json");
    if (!response.ok) return {};
    return await response.json();
  } catch {
    return {};
  }
}

function compatLabel(status) {
  switch (status) {
    case "compatible":
      return "兼容";
    case "incompatible":
      return "不兼容";
    default:
      return "未知";
  }
}

function compatClass(status) {
  switch (status) {
    case "compatible":
      return "ok";
    case "incompatible":
      return "err";
    default:
      return "warn";
  }
}

function upgradeStatusLabel(status) {
  switch (status) {
    case "up_to_date":
      return "已是最新";
    case "update_available":
      return "可升级";
    default:
      return "未知";
  }
}

function upgradeStatusClass(status) {
  switch (status) {
    case "up_to_date":
      return "ok";
    case "update_available":
      return "warn";
    default:
      return "warn";
  }
}

async function renderUpgrade(content, rpc, app, state, go) {
  const toolbar = document.createElement("div");
  toolbar.className = "toolbar row-item";
  toolbar.innerHTML = `
    <input id="upgrade-search" type="search" placeholder="搜索模块…" value="${escapeHtml(state.upgradeQuery || "")}" />
    <button type="button" class="ghost" id="upgrade-search-btn">搜索</button>
  `;
  content.appendChild(toolbar);

  const summaryBody = document.createElement("div");
  content.appendChild(summaryBody);
  const listBody = document.createElement("div");
  content.appendChild(listBody);
  const detailBody = document.createElement("div");
  content.appendChild(detailBody);

  async function loadCheck() {
    summaryBody.innerHTML = "<div class='muted'>检查 release set 模块 pin…</div>";
    listBody.innerHTML = "";
    detailBody.innerHTML = "";
    const body = await rpc.read("upgrade", "check", {
      query: state.upgradeQuery || undefined,
    });
    const releaseSet = body?.release_set || "—";
    const updateCount = body?.update_count ?? 0;
    summaryBody.innerHTML = `
      <div class="card">
        <h2>Release set · ${escapeHtml(releaseSet)}</h2>
        <div class="toolbar nested">
          <span class="pill ${updateCount > 0 ? "warn" : "ok"}">${updateCount} 个模块可升级</span>
        </div>
        <div class="muted">流程：检查 → Git 获取 → 编译 → ABI/pin 更新 → 重载/重启</div>
      </div>
    `;
    const modules = body?.modules || [];
    if (!modules.length) {
      listBody.innerHTML = "<div class='muted'>没有匹配的模块</div>";
      return;
    }
    listBody.innerHTML = modules
      .map(
        (module) => `
      <div class="tree-item row-item upgrade-row">
        <div>
          <strong>${escapeHtml(module.id)}</strong>
          <div class="muted">${escapeHtml(module.kind || "")} · ${escapeHtml(module.url || "")}</div>
          <div class="muted">pin ${escapeHtml(module.pinned_revision || "—")}${module.remote_revision ? ` → 远端 ${escapeHtml(module.remote_revision)}` : ""}</div>
        </div>
        <div class="row-actions">
          <span class="pill ${upgradeStatusClass(module.status)}">${upgradeStatusLabel(module.status)}</span>
          <button type="button" class="ghost" data-plan="${escapeHtml(module.id)}" data-target="${escapeHtml(module.remote_revision || module.pinned_revision || "")}">升级计划</button>
        </div>
      </div>`,
      )
      .join("");
    listBody.querySelectorAll("[data-plan]").forEach((btn) => {
      btn.onclick = () =>
        openPlan(
          btn.getAttribute("data-plan"),
          btn.getAttribute("data-target") || undefined,
        );
    });
  }

  async function openPlan(moduleId, targetRevision) {
    detailBody.innerHTML = "<div class='card'><h2>升级计划</h2><div class='muted'>生成中…</div></div>";
    const params = { module_id: moduleId };
    if (targetRevision) params.target_revision = targetRevision;
    const body = await rpc.read("upgrade", "plan", params);
    const plan = body?.plan || {};
    const steps = plan.steps || [];
    const cliCommand = body?.cli_command || "";
    detailBody.innerHTML = `
      <div class="card">
        <h2>${escapeHtml(moduleId)}</h2>
        <div class="muted">目标 revision · ${escapeHtml(plan.target_revision || "—")}</div>
        <div class="muted">当前 pin · ${escapeHtml(plan.pinned_revision || "—")}</div>
      </div>
      <div class="card">
        <h3>升级步骤（CLI 执行）</h3>
        ${steps
          .map(
            (step, index) =>
              `<div class="tree-item"><strong>${index + 1}. ${escapeHtml(step.title || step.id)}</strong><div class="muted">${escapeHtml(step.detail || "")}</div>${step.cli_hint ? `<pre class="log-block mono">${escapeHtml(step.cli_hint)}</pre>` : ""}</div>`,
          )
          .join("") || "<div class='muted'>暂无</div>"}
        ${cliCommand ? `<pre class="log-block mono" id="upgrade-cli-command">${escapeHtml(cliCommand)}</pre>` : ""}
        <div class="toolbar nested">
          <button type="button" class="ghost" id="copy-upgrade-cli">复制 execute 命令</button>
          <button type="button" class="ghost" id="preview-upgrade-execute">预览 dry-run</button>
          <button type="button" class="ghost" id="goto-plugins">ABI 更新后去插件页重载</button>
        </div>
        <div id="upgrade-execute-preview"></div>
      </div>
    `;
    detailBody.querySelector("#copy-upgrade-cli")?.addEventListener("click", async () => {
      if (!cliCommand) return;
      try {
        await navigator.clipboard.writeText(cliCommand);
      } catch {
        window.prompt("复制 execute 命令", cliCommand);
      }
    });
    detailBody.querySelector("#preview-upgrade-execute")?.addEventListener("click", async () => {
      const preview = detailBody.querySelector("#upgrade-execute-preview");
      if (!preview) return;
      preview.innerHTML = "<div class='muted'>dry-run 预览中…</div>";
      try {
        const executeParams = { module_id: moduleId, dry_run: true };
        if (targetRevision) executeParams.target_revision = targetRevision;
        const executeBody = await rpc.read("upgrade", "execute", executeParams);
        const report = executeBody?.report || {};
        const reportSteps = report.steps || [];
        preview.innerHTML = `
          <div class="section nested">
            <h4>dry-run 结果</h4>
            <div class="muted">${escapeHtml(executeBody?.note || "")}</div>
            ${reportSteps
              .map(
                (step) =>
                  `<div class="tree-item"><strong>${escapeHtml(step.id)} · ${escapeHtml(step.status || "")}</strong><div class="muted">${escapeHtml(step.detail || "")}</div></div>`,
              )
              .join("") || "<div class='muted'>暂无</div>"}
          </div>`;
      } catch (error) {
        preview.innerHTML = `<div class="err">${escapeHtml(String(error))}</div>`;
      }
    });
    detailBody.querySelector("#goto-plugins")?.addEventListener("click", () => {
      if (typeof go === "function") go("plugins");
      else {
        state.page = navigate("plugins").page;
        const contentEl = app.querySelector("#content");
        contentEl.innerHTML = "";
        renderPlugins(contentEl, rpc, app);
      }
    });
  }

  toolbar.querySelector("#upgrade-search-btn").onclick = async () => {
    state.upgradeQuery = toolbar.querySelector("#upgrade-search").value.trim();
    await loadCheck();
  };
  toolbar.querySelector("#upgrade-search").addEventListener("keydown", async (ev) => {
    if (ev.key === "Enter") {
      state.upgradeQuery = ev.target.value.trim();
      await loadCheck();
    }
  });

  await loadCheck();
}

export function applyConsoleTheme(preferred) {
  const theme =
    preferred === "light" || preferred === "dark"
      ? preferred
      : document.documentElement.dataset.theme === "light"
        ? "light"
        : "dark";
  if (theme === "light") document.documentElement.dataset.theme = "light";
  else delete document.documentElement.dataset.theme;
  document.documentElement.style.colorScheme = theme;
}

function ensureMutsukiUiStylesheet() {
  if (document.querySelector('link[data-mutsuki-ui="1"]')) return;
  const existing = [...document.styleSheets].some((sheet) => {
    try {
      return sheet.href && sheet.href.includes("mutsuki-ui.css");
    } catch {
      return false;
    }
  });
  if (existing || document.querySelector('link[href$="mutsuki-ui.css"]')) return;
  const link = document.createElement("link");
  link.rel = "stylesheet";
  link.href = "./mutsuki-ui.css";
  link.dataset.mutsukiUi = "1";
  document.head.appendChild(link);
}

export function mountConsole(el, rpc, options = {}) {
  el.innerHTML = "";
  applyConsoleTheme(options.theme);
  ensureMutsukiUiStylesheet();
  const includeConfig =
    options.includeConfig === true ||
    globalThis.__MUTSUKI_CONSOLE__?.includeConfig === true;
  const includeUpgrade =
    options.includeUpgrade === true ||
    globalThis.__MUTSUKI_CONSOLE__?.includeUpgrade === true;
  const includeBilibili =
    options.includeBilibili === true ||
    globalThis.__MUTSUKI_CONSOLE__?.includeBilibili === true;
  const includeQq =
    options.includeQq === true ||
    globalThis.__MUTSUKI_CONSOLE__?.includeQq === true;
  const includeAgentConnections =
    options.includeAgentConnections === true ||
    globalThis.__MUTSUKI_CONSOLE__?.includeAgentConnections === true;
  const includeBotFlow =
    options.includeBotFlow === true ||
    globalThis.__MUTSUKI_CONSOLE__?.includeBotFlow === true;
  const builtinDatabases =
    options.builtinDatabases ||
    globalThis.__MUTSUKI_CONSOLE__?.builtinDatabases ||
    [];
  el.appendChild(createShell(rpc, { includeConfig, includeUpgrade, includeBilibili, includeQq, includeAgentConnections, includeBotFlow, builtinDatabases }));
}

export default {
  id: "overview",
  setup(ctx) {
    ctx.navigation.register({
      id: "overview.nav",
      label: "概览",
      path: "/",
      order: 1,
      requiredCapability: "runtime.read",
    });
    ctx.pages.register({
      id: "overview.page",
      path: "/",
      title: "概览",
      component: {
        mount(el) {
          mountConsole(el, ctx.rpc);
        },
      },
      requiredCapability: "runtime.read",
    });
  },
};
