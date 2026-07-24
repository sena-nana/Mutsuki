/**
 * Default config WebExtension — Level 1 auto-form + Level 2 format renderer registry.
 */

const rendererRegistry = new Map();

export function registerConfigRenderer(format, renderer) {
  if (!format || typeof renderer !== "function") {
    throw new Error("registerConfigRenderer requires format and render(fn)");
  }
  rendererRegistry.set(String(format), renderer);
}

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

function defaultContext(schema) {
  const scope = schema?.scopes?.[0] || "plugin_instance";
  if (scope === "global") return { scope: "global" };
  return { scope: "plugin_instance", plugin_instance_id: "default" };
}

function wireToPlain(v) {
  if (!v || typeof v !== "object") return v;
  if (
    v.type === "bool" ||
    v.type === "integer" ||
    v.type === "float" ||
    v.type === "string"
  ) {
    return v.value;
  }
  if (v.type === "secret") return v.value;
  if (v.type === "array") return (v.value || []).map(wireToPlain);
  if (v.type === "object") {
    const out = {};
    for (const [k, child] of Object.entries(v.value || {})) out[k] = wireToPlain(child);
    return out;
  }
  return v;
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

function plainToWire(node, plain) {
  const kind = node.value_type?.kind;
  if (kind === "secret" || node.presentation?.secret) {
    return plain || { state: "keep" };
  }
  if (kind === "bool") return { type: "bool", value: !!plain };
  if (kind === "integer") return { type: "integer", value: Number(plain || 0) };
  if (kind === "float") return { type: "float", value: Number(plain || 0) };
  if (kind === "enum") {
    if (node.value_type.multi) {
      const items = Array.isArray(plain) ? plain : [];
      return { type: "array", value: items.map((v) => ({ type: "string", value: String(v) })) };
    }
    return { type: "string", value: String(plain ?? "") };
  }
  if (kind === "array") {
    const items = Array.isArray(plain) ? plain : [];
    const itemNode = { value_type: node.value_type.item, presentation: {}, key: "item" };
    return { type: "array", value: items.map((item) => plainToWire(itemNode, item)) };
  }
  if (kind === "object") {
    const obj = {};
    const source = plain && typeof plain === "object" ? plain : {};
    for (const child of node.children || []) {
      obj[child.key] = plainToWire(child, source[child.key]);
    }
    return { type: "object", value: obj };
  }
  if (kind === "map") {
    const obj = {};
    const source = plain && typeof plain === "object" ? plain : {};
    const valueNode = { value_type: node.value_type.value, presentation: {}, key: "value" };
    for (const [k, v] of Object.entries(source)) {
      obj[k] = plainToWire(valueNode, v);
    }
    return { type: "object", value: obj };
  }
  if (kind === "file_ref" || kind === "directory_ref") {
    return { type: "string", value: String(plain ?? "") };
  }
  return { type: "string", value: String(plain ?? "") };
}

function draftToCandidate(draft, schema) {
  const obj = {};
  for (const node of schema.root.children || []) {
    obj[node.key] = plainToWire(node, draft[node.key]);
  }
  return { type: "object", value: obj };
}

function appendFieldChrome(wrap, node) {
  const title = document.createElement("div");
  title.className = "field-title";
  title.textContent = node.title?.default || node.key;
  if (node.presentation?.unit) title.textContent += ` (${node.presentation.unit})`;
  if (node.presentation?.format) {
    const fmt = document.createElement("span");
    fmt.className = "field-format";
    fmt.textContent = ` · ${node.presentation.format}`;
    title.appendChild(fmt);
  }
  wrap.appendChild(title);
  if (node.description?.default) {
    const help = document.createElement("div");
    help.className = "field-help";
    help.textContent = node.description.default;
    wrap.appendChild(help);
  }
  if (node.restart_policy && node.restart_policy !== "none") {
    const restart = document.createElement("div");
    restart.className = "field-restart";
    restart.textContent = `变更后：${node.restart_policy}`;
    wrap.appendChild(restart);
  }
}

function buildNodeInput(node, draft, key, onChange) {
  const kind = node.value_type?.kind;
  const format = node.presentation?.format;
  if (format && rendererRegistry.has(format)) {
    const host = document.createElement("div");
    host.className = "custom-renderer";
    rendererRegistry.get(format)({
      node,
      value: draft[key],
      setValue: (next) => {
        draft[key] = next;
        onChange();
      },
      host,
    });
    return host;
  }

  if (kind === "bool") {
    const input = document.createElement("input");
    input.type = "checkbox";
    input.checked = !!draft[key];
    input.disabled = node.mutability === "read_only";
    input.addEventListener("change", () => {
      draft[key] = input.checked;
      onChange();
    });
    return input;
  }

  if (kind === "integer" || kind === "float") {
    const input = document.createElement("input");
    input.type = "number";
    input.value = draft[key] ?? node.default_value?.value ?? "";
    input.disabled = node.mutability === "read_only";
    input.addEventListener("change", () => {
      draft[key] = kind === "integer" ? parseInt(input.value, 10) : Number(input.value);
      onChange();
    });
    return input;
  }

  if (kind === "enum") {
    const multi = !!node.value_type.multi;
    const options = node.value_type.options || [];
    if (multi) {
      const box = document.createElement("div");
      box.className = "enum-multi";
      const selected = new Set(Array.isArray(draft[key]) ? draft[key] : []);
      for (const opt of options) {
        const row = document.createElement("label");
        const input = document.createElement("input");
        input.type = "checkbox";
        input.checked = selected.has(opt.value);
        input.disabled = node.mutability === "read_only";
        input.addEventListener("change", () => {
          if (input.checked) selected.add(opt.value);
          else selected.delete(opt.value);
          draft[key] = [...selected];
          onChange();
        });
        row.append(input, document.createTextNode(opt.title?.default || opt.value));
        box.appendChild(row);
      }
      return box;
    }
    const select = document.createElement("select");
    select.disabled = node.mutability === "read_only";
    for (const opt of options) {
      const option = document.createElement("option");
      option.value = opt.value;
      option.textContent = opt.title?.default || opt.value;
      select.appendChild(option);
    }
    select.value = draft[key] ?? options[0]?.value ?? "";
    select.addEventListener("change", () => {
      draft[key] = select.value;
      onChange();
    });
    return select;
  }

  if (kind === "secret" || node.presentation?.secret) {
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
    return row;
  }

  if (kind === "array") {
    if (!Array.isArray(draft[key])) draft[key] = [];
    const box = document.createElement("div");
    box.className = "array-editor";
    draft[key].forEach((item, index) => {
      const row = document.createElement("div");
      row.className = "array-row";
      const itemNode = {
        key: String(index),
        value_type: node.value_type.item,
        presentation: {},
        mutability: node.mutability,
      };
      const itemDraft = { [String(index)]: item };
      const input = buildNodeInput(itemNode, itemDraft, String(index), () => {
        draft[key][index] = itemDraft[String(index)];
        onChange();
      });
      const remove = document.createElement("button");
      remove.type = "button";
      remove.textContent = "删除";
      remove.onclick = () => {
        draft[key].splice(index, 1);
        onChange();
      };
      row.append(input, remove);
      box.appendChild(row);
    });
    const add = document.createElement("button");
    add.type = "button";
    add.textContent = "添加";
    add.onclick = () => {
      draft[key].push("");
      onChange();
    };
    box.appendChild(add);
    return box;
  }

  if (kind === "object") {
    if (!draft[key] || typeof draft[key] !== "object") draft[key] = {};
    const nested = document.createElement("div");
    nested.className = "nested-object";
    for (const child of node.children || []) {
      if (!isVisible(child, draft[key])) continue;
      const wrap = document.createElement("label");
      wrap.className = "field nested";
      appendFieldChrome(wrap, child);
      wrap.appendChild(
        buildNodeInput(child, draft[key], child.key, () => {
          onChange();
        }),
      );
      nested.appendChild(wrap);
    }
    return nested;
  }

  if (kind === "map") {
    if (!draft[key] || typeof draft[key] !== "object") draft[key] = {};
    const box = document.createElement("div");
    box.className = "map-editor";
    for (const [mapKey, mapValue] of Object.entries(draft[key])) {
      const row = document.createElement("div");
      row.className = "map-row";
      const keyInput = document.createElement("input");
      keyInput.value = mapKey;
      keyInput.placeholder = "key";
      const valueNode = {
        key: mapKey,
        value_type: node.value_type.value,
        presentation: {},
        mutability: node.mutability,
      };
      const valueDraft = { [mapKey]: mapValue };
      const valueInput = buildNodeInput(valueNode, valueDraft, mapKey, () => {
        draft[key][mapKey] = valueDraft[mapKey];
        onChange();
      });
      const remove = document.createElement("button");
      remove.type = "button";
      remove.textContent = "删除";
      remove.onclick = () => {
        delete draft[key][mapKey];
        onChange();
      };
      keyInput.addEventListener("change", () => {
        const nextKey = keyInput.value.trim();
        if (!nextKey || nextKey === mapKey) return;
        draft[key][nextKey] = draft[key][mapKey];
        delete draft[key][mapKey];
        onChange();
      });
      row.append(keyInput, valueInput, remove);
      box.appendChild(row);
    }
    const add = document.createElement("button");
    add.type = "button";
    add.textContent = "添加条目";
    add.onclick = () => {
      let i = 1;
      while (draft[key][`key${i}`] != null) i += 1;
      draft[key][`key${i}`] = "";
      onChange();
    };
    box.appendChild(add);
    return box;
  }

  if (kind === "file_ref" || kind === "directory_ref") {
    const input = document.createElement("input");
    input.type = "text";
    input.placeholder = kind === "directory_ref" ? "目录路径" : "文件路径";
    input.value = draft[key] ?? "";
    input.disabled = node.mutability === "read_only";
    input.addEventListener("change", () => {
      draft[key] = input.value;
      onChange();
    });
    return input;
  }

  const input = document.createElement(node.value_type?.multiline ? "textarea" : "input");
  if (input.tagName === "INPUT") input.type = "text";
  input.value = draft[key] ?? node.default_value?.value ?? "";
  input.disabled = node.mutability === "read_only";
  input.addEventListener("change", () => {
    draft[key] = input.value;
    onChange();
  });
  return input;
}

function buildForm(schema, draft, onChange) {
  const root = document.createElement("div");
  root.className = "config-form";
  for (const node of schema.root.children || []) {
    if (!isVisible(node, draft)) continue;
    const wrap = document.createElement("label");
    wrap.className = "field";
    appendFieldChrome(wrap, node);
    wrap.appendChild(buildNodeInput(node, draft, node.key, onChange));
    root.appendChild(wrap);
  }
  return root;
}

function createConsoleApp(rpc) {
  const state = {
    providers: [],
    selected: null,
    schema: null,
    snapshot: null,
    draft: {},
    message: "",
    conflict: null,
  };

  const app = document.createElement("div");
  app.className = "mutsuki-console lilia-workspace";
  app.dataset.liliaSurfaceMode = "solid";
  app.dataset.liliaSurfaceLevel = "base";
  app.innerHTML = `
    <aside class="lilia-workspace-region" data-region="navigation" data-region-separator="inline">
      <div class="secondary-panel">
        <div class="secondary-panel__top">
          <div class="brand">Mutsuki</div>
        </div>
        <nav class="secondary-panel__body sb-section nav" aria-label="Console">
          <a class="sb-tree__row lilia-interactive-item" href="?page=overview"><span class="sb-tree__name">概览</span></a>
          <button type="button" data-route="config" class="sb-tree__row lilia-interactive-item is-active" aria-current="page" data-lilia-selected="true"><span class="sb-tree__name">配置</span></button>
        </nav>
        <div class="secondary-panel__footer sidebar-footer">bot console</div>
      </div>
    </aside>
    <main class="lilia-workspace-region" data-region="main">
      <div class="lilia-workspace-region__content page-scroll">
        <div class="page-header">
          <div>
            <h1>配置</h1>
            <p>由 ConfigDescriptor 自动生成表单</p>
          </div>
        </div>
        <section id="content" class="page-body"></section>
      </div>
    </main>
  `;

  async function refreshProviders() {
    const list = await rpc.call("config", "providers.list", { capabilities: ["*"] });
    state.providers = normalizeProviders(list);
    render();
  }

  async function openProvider(id) {
    state.selected = id;
    state.conflict = null;
    state.schema = await rpc.call("config", "schema.get", {
      provider_id: id,
      capabilities: ["*"],
    });
    const context = defaultContext(state.schema);
    state.snapshot = await rpc.call("config", "snapshot.read", {
      provider_id: id,
      context,
      capabilities: ["*"],
    });
    state.draft = snapshotToDraft(state.snapshot?.value);
    render();
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

    if (state.conflict) {
      const banner = document.createElement("div");
      banner.className = "conflict";
      banner.textContent = `配置已被其他客户端更新（当前 revision=${state.conflict.current}）。请重新加载后再提交。`;
      const reload = document.createElement("button");
      reload.textContent = "重新加载";
      reload.onclick = () => openProvider(state.selected);
      banner.appendChild(reload);
      panel.appendChild(banner);
    }

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
      const context = defaultContext(state.schema);
      const result = await rpc.call("config", "validate", {
        provider_id: state.selected,
        candidate: draftToCandidate(state.draft, state.schema),
        context,
        capabilities: ["*"],
      });
      state.message = result.ok ? "验证通过" : JSON.stringify(result.issues || result);
      renderMessage();
    };
    const applyBtn = document.createElement("button");
    applyBtn.className = "primary";
    applyBtn.textContent = "应用";
    applyBtn.onclick = async () => {
      try {
        const context = defaultContext(state.schema);
        const result = await rpc.call("config", "apply", {
          provider_id: state.selected,
          context,
          capabilities: ["*"],
          request: {
            candidate: draftToCandidate(state.draft, state.schema),
            expected_revision: state.snapshot?.revision ?? 1,
            dry_run: false,
          },
        });
        state.conflict = null;
        const pending = (result.pending_actions || []).join(", ");
        const done = (result.actions || []).join(", ");
        state.message = `已应用 revision=${result.revision}; actions=${done}${
          pending ? `; pending=${pending}` : ""
        }`;
        await openProvider(state.selected);
        renderMessage();
      } catch (error) {
        const text = String(error?.message || error);
        try {
          const parsed = JSON.parse(text);
          if (parsed.kind === "revision_conflict") {
            state.conflict = parsed;
            state.message = "revision 冲突：请重新加载";
            render();
            return;
          }
        } catch {
          /* not structured */
        }
        state.message = text;
        renderMessage();
      }
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

  if (typeof rpc.subscribe === "function") {
    rpc.subscribe("config.revision_changed", (payload) => {
      if (!state.selected) return;
      const provider = payload?.provider_id?.value || payload?.provider_id;
      if (provider && provider !== state.selected) return;
      const remote = payload?.revision?.value ?? payload?.revision;
      const local = state.snapshot?.revision;
      if (remote != null && local != null && Number(remote) !== Number(local)) {
        state.conflict = { current: remote, expected: local };
        state.message = "检测到外部 revision 变更";
        render();
      }
    });
  }

  refreshProviders();
  return app;
}

export class SimpleRpc {
  constructor(url) {
    this.url = url;
    this.ws = null;
    this.pending = new Map();
    this.eventHandlers = new Map();
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
          return;
        }
        if (msg.type === "event") {
          const handlers = this.eventHandlers.get(msg.topic) || [];
          for (const handler of handlers) handler(msg.payload);
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
  subscribe(topic, handler) {
    if (!this.eventHandlers.has(topic)) this.eventHandlers.set(topic, []);
    this.eventHandlers.get(topic).push(handler);
    const subscription_id = crypto.randomUUID();
    this.ws.send(
      JSON.stringify({
        type: "subscribe",
        subscription_id,
        topic,
      }),
    );
    return subscription_id;
  }
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
  if (document.querySelector('link[href$="mutsuki-ui.css"]')) return;
  const link = document.createElement("link");
  link.rel = "stylesheet";
  link.href = "./mutsuki-ui.css";
  document.head.appendChild(link);
}

export function mountConfigConsole(el, rpc) {
  el.innerHTML = "";
  applyConsoleTheme();
  ensureMutsukiUiStylesheet();
  el.appendChild(createConsoleApp(rpc));
}

export default {
  id: "config",
  setup(ctx) {
    ctx.config = ctx.config || {};
    ctx.config.renderers = {
      register(entry) {
        registerConfigRenderer(entry.format, entry.component || entry.render);
      },
    };
    ctx.config.renderers.register({
      format: "cron-expression",
      render({ value, setValue, host }) {
        const input = document.createElement("textarea");
        input.rows = 2;
        input.placeholder = "cron expression";
        input.value = value ?? "";
        input.addEventListener("change", () => setValue(input.value));
        host.appendChild(input);
      },
    });
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
