/**
 * Mutsuki Bot Web Console shell: overview, plugins, runners, event sources, logs.
 */

const READ_CAPS = ["runtime.read"];
const WRITE_CAPS = ["runtime.read", "runtime.write"];

const PAGES = [
  { id: "overview", label: "概览" },
  { id: "upgrade", label: "自动升级", optional: true },
  { id: "plugins", label: "插件" },
  { id: "runners", label: "Runners" },
  { id: "events", label: "EventSources" },
  { id: "tasks", label: "任务" },
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

function currentPage() {
  return new URLSearchParams(location.search).get("page") || "overview";
}

function navigate(page) {
  const url = new URL(location.href);
  if (page === "overview") url.searchParams.delete("page");
  else url.searchParams.set("page", page);
  history.pushState({}, "", url);
  return page;
}

export class SimpleRpc {
  constructor(url, options = {}) {
    this.url = url;
    this.capabilities = options.capabilities || READ_CAPS;
    this.authToken = options.authToken || "local-dev";
    this.ws = null;
    this.pending = new Map();
  }

  async connect() {
    await new Promise((resolve, reject) => {
      this.ws = new WebSocket(this.url);
      this.ws.addEventListener("open", () => {
        this.ws.send(
          JSON.stringify({
            type: "hello",
            protocol_version: "1.0.0",
            capabilities: this.capabilities,
            auth_token: this.authToken,
          }),
        );
      });
      this.ws.addEventListener("message", (ev) => {
        const msg = JSON.parse(String(ev.data));
        if (msg.type === "hello_ack") return resolve(msg);
        if (msg.type === "rpc_result") {
          const p = this.pending.get(msg.id);
          if (!p) return;
          this.pending.delete(msg.id);
          if (msg.error) {
            const err = new Error(msg.error.message || "rpc failed");
            err.code = msg.error.code;
            p.reject(err);
          } else p.resolve(msg.result);
        }
      });
      this.ws.addEventListener("error", reject);
    });
  }

  call(namespace, method, params = {}, capabilities = this.capabilities) {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.ws.send(
        JSON.stringify({
          type: "rpc",
          id,
          namespace,
          method,
          params: { capabilities, ...params },
        }),
      );
    });
  }

  /** @param {unknown} err */
  static formatError(err) {
    if (err && typeof err === "object" && "code" in err && err.code) {
      const code = String(err.code);
      const message = "message" in err && err.message ? String(err.message) : "操作失败";
      return `[${code}] ${message}`;
    }
    if (err instanceof Error) return err.message;
    return String(err);
  }

  read(namespace, method, params = {}) {
    return this.call(namespace, method, params, READ_CAPS);
  }

  write(namespace, method, params = {}) {
    return this.call(namespace, method, params, WRITE_CAPS);
  }
}

function createShell(rpc, options = {}) {
  const includeConfig = options.includeConfig === true;
  const includeUpgrade = options.includeUpgrade === true;
  const state = { page: currentPage(), error: "", busy: false, upgradeDetail: null, upgradeQuery: "" };

  const app = document.createElement("div");
  app.className = "mutsuki-console lilia-workspace";
  app.dataset.liliaSurfaceMode = "solid";
  app.dataset.liliaSurfaceLevel = "base";

  function renderNav() {
    const nav = app.querySelector(".nav");
    nav.innerHTML = "";
    for (const page of PAGES) {
      if (page.optional === true && page.id === "upgrade" && !includeUpgrade) continue;
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
        state.page = navigate(page.id);
        renderNav();
        renderPage();
      };
      nav.appendChild(btn);
    }
    if (includeConfig) {
      const link = document.createElement("a");
      link.className = "sb-tree__row lilia-interactive-item";
      link.href = "?page=config";
      link.innerHTML = `<span class="sb-tree__name">配置</span>`;
      nav.appendChild(link);
    }
  }

  async function renderPage() {
    const content = app.querySelector("#content");
    const title = app.querySelector("#page-title");
    const subtitle = app.querySelector("#page-subtitle");
    const pageMeta = PAGES.find((p) => p.id === state.page) || PAGES[0];
    title.textContent = pageMeta.label;
    subtitle.textContent = pageSubtitle(state.page);
    content.className = "page-body";
    content.innerHTML = "";
    state.error = "";
    state.busy = true;
    try {
      if (state.page === "overview") await renderOverview(content, rpc);
      else if (state.page === "upgrade") await renderUpgrade(content, rpc, app, state);
      else if (state.page === "plugins") await renderPlugins(content, rpc, app);
      else if (state.page === "runners") await renderRunners(content, rpc, app);
      else if (state.page === "events") await renderEvents(content, rpc, app);
      else if (state.page === "tasks") await renderTasks(content, rpc, app);
      else if (state.page === "lifecycle") await renderLifecycle(content, rpc, app);
      else if (state.page === "logs") await renderLogs(content, rpc);
      else await renderOverview(content, rpc);
    } catch (err) {
      state.error = SimpleRpc.formatError(err);
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
    state.page = currentPage();
    renderNav();
    renderPage();
  });
  renderPage();
  return app;
}

function pageSubtitle(page) {
  switch (page) {
    case "upgrade":
      return "对照 release set 检查 Mutsuki 模块 Git pin，生成 fetch / build / ABI / pin 升级计划";
    case "plugins":
      return "插件清单与部署偏好";
    case "runners":
      return "Runner 进程状态与运维操作";
    case "events":
      return "EventSource 连接与健康";
    case "tasks":
      return "Task 列表、事件流与调试提交（需 runtime.write）";
    case "lifecycle":
      return "Core drain 与 Service 关闭（强确认 + runtime.write）";
    case "logs":
      return "运行时日志尾部";
    default:
      return "系统状态 · Bot 结构 · 运行时间";
  }
}

async function renderOverview(content, rpc) {
  content.className = "page-body overview-dashboard";
  const d = await rpc.read("overview", "summary");
  const h = d.health || {};
  const c = d.counts || {};
  const tasks = c.tasks || {};
  const active =
    (tasks.ready || 0) + (tasks.running || 0) + (tasks.waiting || 0) + (tasks.blocked || 0);

  appendMetricGrid(content, [
    ["运行时间", formatDuration(d.uptime_ms)],
    ["任务", String(active)],
    ["已提交", String(tasks.submitted_total ?? "—")],
    ["插件", String(c.plugins ?? 0)],
    ["运行器", String(c.runners ?? 0)],
    ["事件源", String(c.event_sources ?? 0)],
  ]);

  const grid = document.createElement("div");
  grid.className = "overview-grid";
  content.appendChild(grid);

  appendKvCard(grid, "系统状态", [
    ["服务", h.service, true],
    ["核心", h.core, true],
    ["插件", h.plugins, true],
    ["运行器", h.runners, true],
    ["事件源", h.event_sources, true],
  ]);
  appendSection(grid, "健康组件", renderComponents(d.components || {}));
  await renderSecretStatusSection(grid, rpc);
}

function appendMetricGrid(content, metrics) {
  const grid = document.createElement("div");
  grid.className = "metric-grid";
  grid.innerHTML = metrics
    .map(
      ([label, value]) =>
        `<div class="metric-card"><div class="metric-label">${escapeHtml(label)}</div><div class="metric-value">${escapeHtml(value)}</div></div>`,
    )
    .join("");
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
  appendSection(content, title, `<ul class="kv">${items}</ul>`);
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
      flash(app, SimpleRpc.formatError(err), true);
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
        flash(app, SimpleRpc.formatError(err), true);
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
      flash(app, SimpleRpc.formatError(err), true);
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
        flash(app, SimpleRpc.formatError(err), true);
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
        flash(app, SimpleRpc.formatError(err), true);
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

async function renderTasks(content, rpc, app) {
  const toolbar = document.createElement("div");
  toolbar.className = "toolbar row-item";
  toolbar.innerHTML = `
    <button type="button" class="ghost" id="tasks-refresh">刷新列表</button>
    <span class="muted">调试提交需 Core 运行且具备 runtime.write</span>
  `;
  content.appendChild(toolbar);

  const listBody = document.createElement("div");
  content.appendChild(listBody);

  const eventsBody = document.createElement("div");
  eventsBody.className = "card";
  eventsBody.innerHTML = `
    <h2>Task 事件</h2>
    <div class="toolbar row-item">
      <label>sequence <input id="task-event-seq" type="number" min="0" value="0" /></label>
      <label>limit <input id="task-event-limit" type="number" min="1" value="16" /></label>
      <button type="button" class="ghost" id="task-events-fetch">拉取</button>
    </div>
    <div id="task-events-output" class="muted">尚未拉取</div>
  `;
  content.appendChild(eventsBody);

  const submitBody = document.createElement("div");
  submitBody.className = "card";
  submitBody.innerHTML = `
    <h2>submit_batch（调试）</h2>
    <p class="muted">提交合法 TaskBatch JSON；空 batch 会被 ServiceHost 拒绝。</p>
    <textarea id="task-submit-json" class="log-block" rows="8">${escapeHtml(DEFAULT_TASK_BATCH_JSON)}</textarea>
    <div class="toolbar">
      <button type="button" class="ghost" id="task-submit-btn">提交 batch</button>
    </div>
    <div id="task-submit-output" class="muted"></div>
  `;
  content.appendChild(submitBody);

  async function loadTasks() {
    listBody.innerHTML = "<div class='muted'>加载任务…</div>";
    const tasks = await rpc.read("control", "task_list");
    listBody.innerHTML = `<h2>任务列表</h2>${renderTaskRows(tasks || [])}`;
    listBody.querySelectorAll("[data-cancel-task]").forEach((btn) => {
      btn.onclick = async () => {
        const id = btn.getAttribute("data-cancel-task");
        if (!confirmAction(`确认取消任务 ${id}？`)) return;
        try {
          await rpc.write("control", "task_cancel", { id });
          flash(app, `任务 ${id} 取消已提交`);
          await loadTasks();
        } catch (err) {
          flash(app, SimpleRpc.formatError(err), true);
        }
      };
    });
  }

  toolbar.querySelector("#tasks-refresh").onclick = () => loadTasks().catch((err) => {
    flash(app, SimpleRpc.formatError(err), true);
  });

  eventsBody.querySelector("#task-events-fetch").onclick = async () => {
    const sequence = Number(eventsBody.querySelector("#task-event-seq").value || 0);
    const limit = Number(eventsBody.querySelector("#task-event-limit").value || 16);
    const output = eventsBody.querySelector("#task-events-output");
    output.textContent = "拉取中…";
    try {
      const page = await rpc.read("control", "task_events_after", { sequence, limit });
      output.innerHTML = `<pre class="log-block">${escapeHtml(JSON.stringify(page, null, 2))}</pre>`;
    } catch (err) {
      output.innerHTML = `<div class="err-text">${escapeHtml(SimpleRpc.formatError(err))}</div>`;
    }
  };

  submitBody.querySelector("#task-submit-btn").onclick = async () => {
    const raw = submitBody.querySelector("#task-submit-json").value;
    const output = submitBody.querySelector("#task-submit-output");
    if (!confirmAction("确认提交 TaskBatch？此操作会进入 Core 调度。")) return;
    try {
      const payload = JSON.parse(raw);
      const result = await rpc.write("control", "task_submit_batch", payload);
      output.innerHTML = `<pre class="log-block">${escapeHtml(JSON.stringify(result, null, 2))}</pre>`;
      flash(app, "TaskBatch 已提交");
      await loadTasks();
    } catch (err) {
      output.innerHTML = `<div class="err-text">${escapeHtml(SimpleRpc.formatError(err))}</div>`;
      flash(app, SimpleRpc.formatError(err), true);
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
      output.innerHTML = `<div class="err-text">${escapeHtml(SimpleRpc.formatError(err))}</div>`;
      flash(app, SimpleRpc.formatError(err), true);
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
      output.innerHTML = `<div class="err-text">${escapeHtml(SimpleRpc.formatError(err))}</div>`;
      flash(app, SimpleRpc.formatError(err), true);
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

async function renderUpgrade(content, rpc, app, state) {
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
      state.page = navigate("plugins");
      const nav = app.querySelector(".nav");
      if (nav) {
        nav.querySelectorAll(".sb-tree__row").forEach((btn) => {
          const isPlugins = btn.dataset.page === "plugins";
          btn.classList.toggle("is-active", isPlugins);
          if (isPlugins) btn.dataset.liliaSelected = "true";
          else delete btn.dataset.liliaSelected;
        });
      }
      const contentEl = app.querySelector("#content");
      contentEl.innerHTML = "";
      renderPlugins(contentEl, rpc, app);
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
  el.appendChild(createShell(rpc, { includeConfig, includeUpgrade }));
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
