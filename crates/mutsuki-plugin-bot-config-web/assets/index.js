/**
 * Default config WebExtension + standalone console.
 * Koishi-like sidebar layout; LiliaUI tokens via lilia-tokens.css.
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

function createConsoleApp(rpc) {
  const state = {
    providers: [],
    selected: null,
    schema: null,
    snapshot: null,
    draft: {},
    message: "",
  };

  const app = document.createElement("div");
  app.className = "mutsuki-console";
  app.innerHTML = `
    <aside class="sidebar">
      <div class="brand">Mutsuki</div>
      <nav class="nav">
        <button data-route="config" class="nav-item active">配置</button>
      </nav>
      <div class="sidebar-footer">LiliaUI · Schema-first</div>
    </aside>
    <main class="workspace">
      <header class="workspace-header">
        <h1>配置</h1>
        <p>由 ConfigDescriptor 自动生成表单</p>
      </header>
      <section id="content" class="workspace-content"></section>
    </main>
  `;

  async function refreshProviders() {
    const list = await rpc.call("config", "providers.list", { capabilities: ["*"] });
    state.providers = normalizeProviders(list);
    render();
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

  function render() {
    const content = app.querySelector("#content");
    content.innerHTML = "";
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

  refreshProviders();
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

export function mountConfigConsole(el, rpc) {
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
.workspace-header h1{margin:0;font-size:1.15rem}
.workspace-header p{margin:.35rem 0 0;color:var(--text-muted);font-size:.85rem}
.workspace-content{padding:1.2rem 1.4rem;overflow:auto}
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
  id: "config",
  setup(ctx) {
    ctx.navigation.register({
      id: "config.nav",
      label: "配置",
      path: "/config",
      order: 10,
      requiredCapability: "config.schema.read",
    });
    ctx.pages.register({
      id: "config.page",
      path: "/config",
      title: "配置",
      component: {
        mount(el) {
          mountConfigConsole(el, ctx.rpc);
        },
      },
      requiredCapability: "config.schema.read",
    });
  },
};
