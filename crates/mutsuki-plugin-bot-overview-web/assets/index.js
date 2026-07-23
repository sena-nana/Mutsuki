/**
 * Overview console: system status, Bot structure, task/runner counts, uptime.
 */

function formatDuration(ms) {
  if (ms == null || Number.isNaN(Number(ms))) return "—";
  const total = Math.max(0, Math.floor(Number(ms) / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
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

function createApp(rpc) {
  const state = { data: null, error: "" };
  const app = document.createElement("div");
  app.className = "mutsuki-console";
  app.innerHTML = `
    <aside class="sidebar">
      <div class="brand">Mutsuki</div>
      <nav class="nav"><button class="nav-item active">概览</button></nav>
      <div class="sidebar-footer">runtime overview</div>
    </aside>
    <main class="workspace">
      <header class="workspace-header">
        <div class="header-row">
          <div><h1>概览</h1><p>系统状态 · Bot 结构 · 运行时间</p></div>
          <button type="button" id="refresh" class="ghost">刷新</button>
        </div>
      </header>
      <section id="content" class="workspace-content"></section>
    </main>
  `;

  async function refresh() {
    state.error = "";
    try {
      state.data = await rpc.call("overview", "summary");
    } catch (err) {
      state.error = err?.message || String(err);
      state.data = null;
    }
    render();
  }

  function render() {
    const content = app.querySelector("#content");
    content.innerHTML = "";
    if (state.error) {
      content.innerHTML = `<div class="error-banner">${state.error}</div>`;
      return;
    }
    if (!state.data) {
      content.textContent = "加载中…";
      return;
    }
    const d = state.data;
    const h = d.health || {};
    const c = d.counts || {};
    const tasks = c.tasks || {};

    const status = document.createElement("div");
    status.className = "card-grid";
    for (const [label, value] of [
      ["Service", h.service],
      ["Core", h.core],
      ["Plugins", h.plugins],
      ["Runners", h.runners],
      ["EventSources", h.event_sources],
    ]) {
      status.innerHTML += `<div class="status-card ${healthClass(value)}"><div class="label">${label}</div><div class="value">${value || "—"}</div></div>`;
    }
    content.appendChild(status);

    const active =
      (tasks.ready || 0) + (tasks.running || 0) + (tasks.waiting || 0) + (tasks.blocked || 0);
    const metrics = document.createElement("div");
    metrics.className = "card-grid";
    for (const [label, value] of [
      ["Uptime", formatDuration(d.uptime_ms)],
      ["Tasks", String(active)],
      ["Submitted", String(tasks.submitted_total ?? "—")],
      ["Plugins", String(c.plugins ?? 0)],
      ["Runners", String(c.runners ?? 0)],
      ["EventSources", String(c.event_sources ?? 0)],
    ]) {
      metrics.innerHTML += `<div class="metric-card"><div class="label">${label}</div><div class="value">${value}</div></div>`;
    }
    content.appendChild(metrics);

    const section = (title, html) => {
      const el = document.createElement("div");
      el.className = "section";
      el.innerHTML = `<h2>${title}</h2>${html}`;
      content.appendChild(el);
    };

    const plugins = d.plugins?.plugins || [];
    section(
      "插件",
      plugins.length
        ? plugins
            .map(
              (p) =>
                `<div class="tree-item"><strong>${p.plugin_id}</strong><div class="muted">active=${p.active_deployment || "—"} · configured=${p.configured}</div></div>`,
            )
            .join("")
        : "<div class='muted'>暂无</div>",
    );

    const runners = d.runners || [];
    section(
      "Runners",
      runners.length
        ? runners
            .map(
              (r) =>
                `<div class="tree-item"><strong>${r.runner_id}</strong><div class="muted">${r.plugin_id} · ${r.state} · pid=${r.pid ?? "—"}</div></div>`,
            )
            .join("")
        : "<div class='muted'>暂无</div>",
    );

    const sources = d.event_sources || [];
    section(
      "EventSources",
      sources.length
        ? sources
            .map(
              (s) =>
                `<div class="tree-item"><strong>${s.source_id}</strong><div class="muted">${s.state}/${s.health} · uptime=${formatDuration(elapsed(s.started_at_unix_ms))}</div></div>`,
            )
            .join("")
        : "<div class='muted'>暂无</div>",
    );

    const comps = d.components || {};
    const ids = Object.keys(comps);
    if (ids.length) {
      section(
        "Health 组件",
        ids
          .map((id) => {
            const snap = comps[id] || {};
            const started = snap.started_at_unix_ms ?? snap.connected_since_unix_ms;
            return `<div class="tree-item"><strong>${id}</strong><div class="muted">status=${snap.status ?? "—"} · uptime=${formatDuration(elapsed(started))}</div></div>`;
          })
          .join(""),
      );
    }
  }

  app.querySelector("#refresh").onclick = refresh;
  refresh();
  setInterval(refresh, 5000);
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
        if (msg.type === "hello_ack") return resolve(msg);
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
  style.textContent = CSS;
  document.head.appendChild(style);
  el.appendChild(createApp(rpc));
}

const CSS = `
html,body,#app{height:100%;margin:0;background:var(--bg);color:var(--text);font-family:var(--font-sans)}
.mutsuki-console{display:flex;height:100%}
.sidebar{width:200px;background:var(--bg-elev);border-right:1px solid var(--border-soft);display:flex;flex-direction:column}
.brand{font-size:1.2rem;font-weight:700;padding:1rem;color:var(--accent)}
.nav{padding:.5rem}.nav-item{background:var(--accent-soft);border:0;color:var(--text);text-align:left;padding:.55rem .75rem;border-radius:6px;width:100%}
.sidebar-footer{margin-top:auto;padding:1rem;color:var(--text-faint);font-size:.75rem}
.workspace{flex:1;display:flex;flex-direction:column;min-width:0}
.workspace-header{padding:1rem 1.3rem;border-bottom:1px solid var(--border-soft)}
.header-row{display:flex;justify-content:space-between;gap:1rem}
.workspace-header h1{margin:0;font-size:1.1rem}
.workspace-header p{margin:.3rem 0 0;color:var(--text-muted);font-size:.85rem}
.workspace-content{padding:1.1rem 1.3rem;overflow:auto}
.ghost{background:var(--bg-subtle);border:1px solid var(--border);color:var(--text);padding:.4rem .7rem;border-radius:6px;cursor:pointer}
.card-grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(130px,1fr));gap:.7rem;margin-bottom:1rem}
.status-card,.metric-card{background:var(--bg-elev);border:1px solid var(--border-soft);border-radius:8px;padding:.8rem}
.status-card.ok{border-color:color-mix(in oklch,var(--ok) 40%,var(--border-soft))}
.status-card.warn{border-color:color-mix(in oklch,var(--accent) 40%,var(--border-soft))}
.status-card.err{border-color:color-mix(in oklch,var(--err) 40%,var(--border-soft))}
.label{font-size:.75rem;color:var(--text-muted);margin-bottom:.3rem}
.value{font-size:1.05rem;font-weight:600}
.section{margin:1.2rem 0}.section h2{margin:0 0 .6rem;font-size:.95rem}
.tree-item{background:var(--bg-elev);border:1px solid var(--border-soft);border-radius:8px;padding:.7rem .85rem;margin-bottom:.45rem}
.muted{color:var(--text-muted);font-size:.8rem;margin-top:.2rem}
.error-banner{background:color-mix(in oklch,var(--err) 16%,var(--bg-elev));border:1px solid var(--err);color:var(--err);padding:.8rem;border-radius:8px}
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
