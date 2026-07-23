/**
 * Overview-first Mutsuki Console shell.
 * Routes: overview (default) + config.
 */

function deepEqual(a, b) {
  return JSON.stringify(a) === JSON.stringify(b);
}

function evalConfigExpr(expr, draft) {
  switch (expr.op) {
    case "field":
      return !!draft[expr.key];
    case "literal": {
      const v = expr.value;
      if (v && typeof v === "object" && "type" in v) return !!v.value;
      return !!v;
    }
    case "eq":
      return deepEqual(atomValue(expr.left, draft), atomValue(expr.right, draft));
    case "ne":
      return !deepEqual(atomValue(expr.left, draft), atomValue(expr.right, draft));
    case "and":
      return (expr.items || []).every((e) => evalConfigExpr(e, draft));
    case "or":
      return (expr.items || []).some((e) => evalConfigExpr(e, draft));
    case "not":
      return !evalConfigExpr(expr.expr, draft);
    case "is_set":
      return draft[expr.key] != null;
    default:
      return true;
  }
}

function atomValue(expr, draft) {
  if (expr.op === "field") return draft[expr.key];
  if (expr.op === "literal") {
    const v = expr.value;
    if (v && typeof v === "object" && "type" in v) return v.value;
    return v;
  }
  return evalConfigExpr(expr, draft);
}

function isVisible(node, draft) {
  if (!node.visibility) return true;
  try {
    return evalConfigExpr(node.visibility, draft);
  } catch {
    return true;
  }
}

function normalizeProviders(list) {
  if (!Array.isArray(list)) return [];
  return list.map((x) => {
    if (typeof x === "string") return x;
    return x.value || x[0] || String(x);
  });
}

function formatDuration(ms) {
  if (ms == null || Number.isNaN(Number(ms))) return "—";
  const total = Math.max(0, Math.floor(Number(ms) / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  if (h > 0) return `${h}h ${m}m ${s}s`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

function elapsedFromUnixMs(startedAt, nowMs = Date.now()) {
  if (startedAt == null) return null;
  return Math.max(0, nowMs - Number(startedAt));
}

function healthClass(value) {
  const v = String(value || "").toLowerCase();
  if (v === "ok" || v === "healthy") return "ok";
  if (v === "degraded") return "warn";
  if (v === "unhealthy" || v === "stopped" || v === "failed") return "err";
  return "muted";
}

function createConsoleApp(rpc) {
  const state = {
    route: "overview",
    overview: null,
    structure: null,
    overviewError: "",
    providers: [],
    selected: null,
    schema: null,
    snapshot: null,
    draft: {},
    message: "",
    configAvailable: true,
  };

  const app = document.createElement("div");
  app.className = "mutsuki-console";
  app.innerHTML = `
    <aside class="sidebar">
      <div class="brand">Mutsuki</div>
      <nav class="nav">
        <button data-route="overview" class="nav-item active">概览</button>
        <button data-route="config" class="nav-item">配置</button>
      </nav>
      <div class="sidebar-footer">LiliaUI · Runtime overview</div>
    </aside>
    <main class="workspace">
      <header class="workspace-header">
        <div class="header-row">
          <div>
            <h1 id="page-title">概览</h1>
            <p id="page-sub">系统状态、Bot 结构与运行时间</p>
          </div>
          <button type="button" id="refresh-btn" class="ghost">刷新</button>
        </div>
      </header>
      <section id="content" class="workspace-content"></section>
    </main>
  `;

  app.querySelectorAll("[data-route]").forEach((btn) => {
    btn.addEventListener("click", () => {
      state.route = btn.getAttribute("data-route");
      app.querySelectorAll("[data-route]").forEach((b) => b.classList.toggle("active", b === btn));
      syncHeader();
      render();
      if (state.route === "overview") refreshOverview();
      if (state.route === "config") refreshProviders();
    });
  });

  app.querySelector("#refresh-btn").addEventListener("click", () => {
    if (state.route === "overview") refreshOverview();
    else refreshProviders();
  });

  function syncHeader() {
    const title = app.querySelector("#page-title");
    const sub = app.querySelector("#page-sub");
    if (state.route === "overview") {
      title.textContent = "概览";
      sub.textContent = "系统状态、Bot 结构与运行时间";
    } else {
      title.textContent = "配置";
      sub.textContent = "由 ConfigDescriptor 自动生成表单";
    }
  }

  async function refreshOverview() {
    state.overviewError = "";
    try {
      const [summary, structure] = await Promise.all([
        rpc.call("overview", "summary"),
        rpc.call("overview", "structure"),
      ]);
      state.overview = summary;
      state.structure = structure;
    } catch (err) {
      state.overviewError = err?.message || String(err);
      state.overview = null;
      state.structure = null;
    }
    if (state.route === "overview") render();
  }

  async function refreshProviders() {
    try {
      const list = await rpc.call("config", "providers.list", { capabilities: ["*"] });
      state.providers = normalizeProviders(list);
      state.configAvailable = true;
    } catch (err) {
      state.providers = [];
      state.configAvailable = false;
      state.message = err?.message || String(err);
    }
    if (state.route === "config") render();
  }

  async function openProvider(id) {
    state.selected = id;
    state.schema = await rpc.call("config", "schema.get", {
      provider_id: id,
      capabilities: ["*"],
    });
    state.snapshot = await rpc.call("config", "snapshot.read", {
      provider_id: id,
      context: { scope: "plugin_instance", plugin_instance_id: "demo" },
      capabilities: ["*"],
    });
    state.draft = snapshotToDraft(state.snapshot?.value);
    render();
  }

  function snapshotToDraft(value) {
    if (!value) return {};
    if (value.type === "object") {
      const out = {};
      for (const [k, v] of Object.entries(value.value || {})) out[k] = wireToPlain(v);
      return out;
    }
    if (typeof value === "object" && !value.type) return { ...value };
    return {};
  }

  function wireToPlain(v) {
    if (!v || typeof v !== "object") return v;
    if (v.type === "bool" || v.type === "integer" || v.type === "float" || v.type === "string") {
      return v.value;
    }
    if (v.type === "secret") return v.value;
    if (v.type === "object") {
      const out = {};
      for (const [k, child] of Object.entries(v.value || {})) out[k] = wireToPlain(child);
      return out;
    }
    return v;
  }

  function draftToCandidate(draft, schema) {
    const obj = {};
    for (const node of schema.root.children || []) {
      const key = node.key;
      const kind = node.value_type?.kind;
      if (kind === "secret" || node.presentation?.secret) {
        obj[key] = draft[key] || { state: "keep" };
      } else if (kind === "bool") {
        obj[key] = { type: "bool", value: !!draft[key] };
      } else if (kind === "integer") {
        obj[key] = { type: "integer", value: Number(draft[key] || 0) };
      } else if (kind === "float") {
        obj[key] = { type: "float", value: Number(draft[key] || 0) };
      } else {
        obj[key] = { type: "string", value: String(draft[key] ?? "") };
      }
    }
    return { type: "object", value: obj };
  }

  function buildForm(schema, draft, onChange) {
    const root = document.createElement("div");
    root.className = "config-form";
    for (const node of schema.root.children || []) {
      if (!isVisible(node, draft)) continue;
      const wrap = document.createElement("label");
      wrap.className = "field";
      const title = document.createElement("div");
      title.className = "field-title";
      title.textContent = node.title?.default || node.key;
      if (node.presentation?.unit) title.textContent += ` (${node.presentation.unit})`;
      wrap.appendChild(title);
      const kind = node.value_type?.kind;
      const key = node.key;
      if (kind === "bool") {
        const input = document.createElement("input");
        input.type = "checkbox";
        input.checked = !!draft[key];
        input.addEventListener("change", () => {
          draft[key] = input.checked;
          onChange();
        });
        wrap.appendChild(input);
      } else if (kind === "integer" || kind === "float") {
        const input = document.createElement("input");
        input.type = "number";
        input.value = draft[key] ?? node.default_value?.value ?? "";
        input.addEventListener("change", () => {
          draft[key] = kind === "integer" ? parseInt(input.value, 10) : Number(input.value);
          onChange();
        });
        wrap.appendChild(input);
      } else if (kind === "secret" || node.presentation?.secret) {
        const row = document.createElement("div");
        row.className = "secret-row";
        const status = document.createElement("span");
        status.textContent = `状态: ${draft[key]?.state || "absent"}`;
        const setBtn = document.createElement("button");
        setBtn.type = "button";
        setBtn.textContent = "设置";
        setBtn.onclick = () => {
          const next = prompt("新密钥");
          if (next == null) return;
          draft[key] = { state: "set", value: next };
          onChange();
        };
        const keepBtn = document.createElement("button");
        keepBtn.type = "button";
        keepBtn.textContent = "保持";
        keepBtn.onclick = () => {
          draft[key] = { state: "keep" };
          onChange();
        };
        const clearBtn = document.createElement("button");
        clearBtn.type = "button";
        clearBtn.className = "danger";
        clearBtn.textContent = "清除";
        clearBtn.onclick = () => {
          draft[key] = { state: "clear" };
          onChange();
        };
        row.append(status, setBtn, keepBtn, clearBtn);
        wrap.appendChild(row);
      } else {
        const input = document.createElement(node.value_type?.multiline ? "textarea" : "input");
        if (input.tagName === "INPUT") input.type = "text";
        input.value = draft[key] ?? node.default_value?.value ?? "";
        input.addEventListener("change", () => {
          draft[key] = input.value;
          onChange();
        });
        wrap.appendChild(input);
      }
      root.appendChild(wrap);
    }
    return root;
  }

  function renderOverview(content) {
    if (state.overviewError) {
      const err = document.createElement("div");
      err.className = "error-banner";
      err.textContent = state.overviewError;
      content.appendChild(err);
      return;
    }
    if (!state.overview) {
      content.textContent = "加载中…";
      return;
    }

    const summary = state.overview;
    const health = summary.health || {};
    const counts = summary.counts || {};
    const tasks = counts.tasks || {};

    const statusRow = document.createElement("div");
    statusRow.className = "card-grid";
    for (const [label, value] of [
      ["Service", health.service],
      ["Core", health.core],
      ["Plugins", health.plugins],
      ["Runners", health.runners],
      ["EventSources", health.event_sources],
    ]) {
      const card = document.createElement("div");
      card.className = `status-card ${healthClass(value)}`;
      card.innerHTML = `<div class="status-label">${label}</div><div class="status-value">${value || "—"}</div>`;
      statusRow.appendChild(card);
    }
    content.appendChild(statusRow);

    const metrics = document.createElement("div");
    metrics.className = "card-grid metrics";
    const taskActive =
      (tasks.ready || 0) + (tasks.running || 0) + (tasks.waiting || 0) + (tasks.blocked || 0);
    for (const [label, value] of [
      ["Uptime", formatDuration(summary.uptime_ms)],
      ["Tasks (active)", String(taskActive)],
      ["Tasks (submitted)", String(tasks.submitted_total ?? "—")],
      ["Plugins", String(counts.plugins ?? 0)],
      ["Runners", String(counts.runners ?? 0)],
      ["EventSources", String(counts.event_sources ?? 0)],
    ]) {
      const card = document.createElement("div");
      card.className = "metric-card";
      card.innerHTML = `<div class="metric-label">${label}</div><div class="metric-value">${value}</div>`;
      metrics.appendChild(card);
    }
    content.appendChild(metrics);

    if (tasks && typeof tasks === "object") {
      const taskBreak = document.createElement("div");
      taskBreak.className = "section";
      taskBreak.innerHTML = `<h2>任务数量</h2>`;
      const list = document.createElement("div");
      list.className = "kv-grid";
      for (const key of [
        "ready",
        "running",
        "waiting",
        "blocked",
        "completed",
        "failed",
        "cancelled",
        "expired",
        "dead_letter",
      ]) {
        const row = document.createElement("div");
        row.className = "kv-row";
        row.innerHTML = `<span>${key}</span><strong>${tasks[key] ?? 0}</strong>`;
        list.appendChild(row);
      }
      taskBreak.appendChild(list);
      content.appendChild(taskBreak);
    }

    const structure = state.structure || {};
    const plugins = structure.plugins?.plugins || [];
    const runners = structure.runners || summary.runners || [];
    const sources = structure.event_sources || summary.event_sources || [];

    const bot = document.createElement("div");
    bot.className = "section";
    bot.innerHTML = `<h2>Bot 结构</h2>`;

    const pluginBlock = document.createElement("div");
    pluginBlock.className = "tree-block";
    pluginBlock.innerHTML = `<h3>插件 (${plugins.length})</h3>`;
    if (!plugins.length) {
      pluginBlock.appendChild(document.createTextNode("暂无插件"));
    } else {
      for (const p of plugins) {
        const item = document.createElement("div");
        item.className = "tree-item";
        const candidates = (p.candidates || [])
          .map((c) => `${c.deployment}${c.available ? "" : " (unavailable)"}`)
          .join(", ");
        item.innerHTML = `<strong>${p.plugin_id}</strong>
          <div class="muted">active=${p.active_deployment || "—"} · preferred=${p.preferred_deployment || "—"} · configured=${p.configured}</div>
          <div class="muted">candidates: ${candidates || "—"}</div>`;
        pluginBlock.appendChild(item);
      }
    }
    bot.appendChild(pluginBlock);

    const runnerBlock = document.createElement("div");
    runnerBlock.className = "tree-block";
    runnerBlock.innerHTML = `<h3>Runners (${runners.length})</h3>`;
    for (const r of runners) {
      const item = document.createElement("div");
      item.className = "tree-item";
      item.innerHTML = `<strong>${r.runner_id}</strong>
        <div class="muted">plugin=${r.plugin_id} · state=${r.state} · pid=${r.pid ?? "—"} · restarts=${r.restarts ?? 0}</div>`;
      runnerBlock.appendChild(item);
    }
    if (!runners.length) runnerBlock.appendChild(document.createTextNode("暂无 Runner"));
    bot.appendChild(runnerBlock);

    const sourceBlock = document.createElement("div");
    sourceBlock.className = "tree-block";
    sourceBlock.innerHTML = `<h3>EventSources (${sources.length})</h3>`;
    for (const s of sources) {
      const item = document.createElement("div");
      item.className = "tree-item";
      const uptime = formatDuration(elapsedFromUnixMs(s.started_at_unix_ms));
      item.innerHTML = `<strong>${s.source_id}</strong>
        <div class="muted">plugin=${s.plugin_id} · state=${s.state} · health=${s.health} · uptime=${uptime}</div>
        <div class="muted">last_event=${s.last_event_unix_ms ?? "—"} · reconnects=${s.reconnects ?? 0}</div>`;
      sourceBlock.appendChild(item);
    }
    if (!sources.length) sourceBlock.appendChild(document.createTextNode("暂无 EventSource"));
    bot.appendChild(sourceBlock);

    const components = structure.components || {};
    const componentIds = structure.component_ids || Object.keys(components);
    if (componentIds.length) {
      const compBlock = document.createElement("div");
      compBlock.className = "tree-block";
      compBlock.innerHTML = `<h3>Health 组件</h3>`;
      for (const id of componentIds) {
        const snap = components[id] || {};
        const item = document.createElement("div");
        item.className = "tree-item";
        const started =
          snap.started_at_unix_ms ?? snap.connected_since_unix_ms ?? null;
        const uptime = formatDuration(elapsedFromUnixMs(started));
        item.innerHTML = `<strong>${id}</strong>
          <div class="muted">status=${snap.status ?? "—"} · uptime=${uptime}</div>`;
        compBlock.appendChild(item);
      }
      bot.appendChild(compBlock);
    }

    content.appendChild(bot);

    const uptimeSection = document.createElement("div");
    uptimeSection.className = "section";
    uptimeSection.innerHTML = `<h2>组件运行时间</h2>
      <div class="kv-grid">
        <div class="kv-row"><span>Service</span><strong>${formatDuration(summary.uptime_ms)}</strong></div>
      </div>`;
    content.appendChild(uptimeSection);
  }

  function renderConfig(content) {
    if (!state.configAvailable) {
      const err = document.createElement("div");
      err.className = "error-banner";
      err.textContent = state.message || "配置扩展不可用";
      content.appendChild(err);
      return;
    }
    if (!state.selected) {
      const list = document.createElement("div");
      list.className = "provider-list";
      for (const id of state.providers) {
        const btn = document.createElement("button");
        btn.className = "provider-item";
        btn.textContent = id;
        btn.onclick = () => openProvider(id);
        list.appendChild(btn);
      }
      if (!state.providers.length) {
        list.textContent = "暂无 ConfigProvider。";
      }
      content.appendChild(list);
      return;
    }

    const panel = document.createElement("div");
    const meta = document.createElement("div");
    meta.className = "meta";
    meta.textContent = `${state.selected} · revision ${state.snapshot?.revision ?? "-"}`;
    panel.appendChild(meta);

    const formHost = document.createElement("div");
    const rebuild = () => {
      formHost.innerHTML = "";
      formHost.appendChild(buildForm(state.schema, state.draft, rebuild));
    };
    rebuild();
    panel.appendChild(formHost);

    const actions = document.createElement("div");
    actions.className = "actions";
    const validateBtn = document.createElement("button");
    validateBtn.textContent = "验证";
    validateBtn.onclick = async () => {
      const result = await rpc.call("config", "validate", {
        provider_id: state.selected,
        candidate: draftToCandidate(state.draft, state.schema),
        context: { scope: "plugin_instance", plugin_instance_id: "demo" },
        capabilities: ["*"],
      });
      state.message = result.ok ? "验证通过" : JSON.stringify(result.issues || result);
      renderMessage();
    };
    const applyBtn = document.createElement("button");
    applyBtn.className = "primary";
    applyBtn.textContent = "应用";
    applyBtn.onclick = async () => {
      const result = await rpc.call("config", "apply", {
        provider_id: state.selected,
        context: { scope: "plugin_instance", plugin_instance_id: "demo" },
        capabilities: ["*"],
        request: {
          candidate: draftToCandidate(state.draft, state.schema),
          expected_revision: state.snapshot?.revision ?? 1,
          dry_run: false,
        },
      });
      state.message = `已应用 revision=${result.revision}`;
      await openProvider(state.selected);
      renderMessage();
    };
    const backBtn = document.createElement("button");
    backBtn.textContent = "返回";
    backBtn.onclick = () => {
      state.selected = null;
      render();
    };
    actions.append(backBtn, validateBtn, applyBtn);
    panel.appendChild(actions);
    const msg = document.createElement("div");
    msg.id = "message";
    msg.className = "message";
    msg.textContent = state.message;
    panel.appendChild(msg);
    content.appendChild(panel);

    function renderMessage() {
      const el = panel.querySelector("#message");
      if (el) el.textContent = state.message;
    }
  }

  function render() {
    const content = app.querySelector("#content");
    content.innerHTML = "";
    if (state.route === "overview") renderOverview(content);
    else renderConfig(content);
  }

  refreshOverview();
  setInterval(() => {
    if (state.route === "overview") refreshOverview();
  }, 5000);

  return app;
}

export class SimpleRpc {
  constructor(url) {
    this.url = url;
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
            capabilities: [],
            auth_token: "local-dev",
          }),
        );
      });
      this.ws.addEventListener("message", (ev) => {
        const msg = JSON.parse(String(ev.data));
        if (msg.type === "hello_ack") {
          resolve(msg);
          return;
        }
        if (msg.type === "rpc_result") {
          const p = this.pending.get(msg.id);
          if (!p) return;
          this.pending.delete(msg.id);
          if (msg.error) p.reject(new Error(msg.error.message || "rpc failed"));
          else p.resolve(msg.result);
        }
      });
      this.ws.addEventListener("error", reject);
    });
  }
  call(namespace, method, params = {}) {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.ws.send(JSON.stringify({ type: "rpc", id, namespace, method, params }));
    });
  }
}

export function mountConsole(el, rpc) {
  el.innerHTML = "";
  const link = document.createElement("link");
  link.rel = "stylesheet";
  link.href = "./lilia-tokens.css";
  document.head.appendChild(link);
  const style = document.createElement("style");
  style.textContent = CONSOLE_CSS;
  document.head.appendChild(style);
  el.appendChild(createConsoleApp(rpc));
}

/** @deprecated use mountConsole */
export function mountConfigConsole(el, rpc) {
  mountConsole(el, rpc);
}

const CONSOLE_CSS = `
html,body,#app{height:100%;margin:0;background:var(--bg);color:var(--text);font-family:var(--font-sans)}
.mutsuki-console{display:flex;height:100%}
.sidebar{width:220px;background:var(--bg-elev);border-right:1px solid var(--border-soft);display:flex;flex-direction:column}
.brand{font-size:1.25rem;font-weight:700;padding:1rem 1.1rem;color:var(--accent)}
.nav{display:flex;flex-direction:column;gap:.25rem;padding:.5rem}
.nav-item{background:transparent;border:0;color:var(--text-muted);text-align:left;padding:.55rem .75rem;border-radius:6px;cursor:pointer}
.nav-item.active{background:var(--accent-soft);color:var(--text)}
.sidebar-footer{margin-top:auto;padding:1rem;color:var(--text-faint);font-size:.75rem}
.workspace{flex:1;display:flex;flex-direction:column;min-width:0}
.workspace-header{padding:1.1rem 1.4rem;border-bottom:1px solid var(--border-soft)}
.header-row{display:flex;align-items:flex-start;justify-content:space-between;gap:1rem}
.workspace-header h1{margin:0;font-size:1.15rem}
.workspace-header p{margin:.35rem 0 0;color:var(--text-muted);font-size:.85rem}
.workspace-content{padding:1.2rem 1.4rem;overflow:auto}
.ghost{background:var(--bg-subtle);border:1px solid var(--border);color:var(--text);padding:.45rem .75rem;border-radius:6px;cursor:pointer}
.card-grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(140px,1fr));gap:.75rem;margin-bottom:1.2rem}
.status-card,.metric-card{background:var(--bg-elev);border:1px solid var(--border-soft);border-radius:8px;padding:.85rem}
.status-card.ok{border-color:color-mix(in oklch,var(--ok) 45%,var(--border-soft))}
.status-card.warn{border-color:color-mix(in oklch,var(--accent) 45%,var(--border-soft))}
.status-card.err{border-color:color-mix(in oklch,var(--err) 45%,var(--border-soft))}
.status-label,.metric-label{font-size:.75rem;color:var(--text-muted);margin-bottom:.35rem}
.status-value,.metric-value{font-size:1.05rem;font-weight:600}
.section{margin:1.4rem 0}
.section h2{margin:0 0 .75rem;font-size:1rem}
.tree-block{margin-bottom:1rem}
.tree-block h3{margin:0 0 .5rem;font-size:.9rem;color:var(--text-muted)}
.tree-item{background:var(--bg-elev);border:1px solid var(--border-soft);border-radius:8px;padding:.75rem .9rem;margin-bottom:.5rem}
.muted{color:var(--text-muted);font-size:.8rem;margin-top:.25rem}
.kv-grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(180px,1fr));gap:.4rem .9rem}
.kv-row{display:flex;justify-content:space-between;gap:.75rem;padding:.35rem 0;border-bottom:1px solid var(--border-soft);font-size:.85rem}
.error-banner{background:color-mix(in oklch,var(--err) 18%,var(--bg-elev));border:1px solid var(--err);color:var(--err);padding:.85rem 1rem;border-radius:8px}
.provider-list{display:flex;flex-direction:column;gap:.5rem;max-width:420px}
.provider-item,.actions button,.secret-row button{background:var(--bg-subtle);border:1px solid var(--border);color:var(--text);padding:.55rem .8rem;border-radius:6px;cursor:pointer}
.actions{display:flex;gap:.5rem;margin-top:1rem}
.actions .primary{background:var(--accent);color:var(--accent-text);border-color:var(--accent)}
.field{display:flex;flex-direction:column;gap:.35rem;margin-bottom:1rem;max-width:480px}
.field-title{font-size:.9rem}
.field input,.field textarea{background:var(--bg-subtle);border:1px solid var(--border);color:var(--text);border-radius:6px;padding:.45rem .6rem}
.secret-row{display:flex;gap:.4rem;align-items:center;flex-wrap:wrap}
.secret-row .danger{color:var(--err)}
.meta{color:var(--text-muted);font-size:.8rem;margin-bottom:1rem}
.message{margin-top:1rem;color:var(--ok)}
`;

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
