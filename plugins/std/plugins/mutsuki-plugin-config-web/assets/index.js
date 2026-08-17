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

function configEditorStore() {
  return (globalThis.__mutsukiConfigEditors ??= new Map());
}

export function registerConfigEditor(entry) {
  if (!entry?.providerId || !entry?.pageId) {
    throw new Error("registerConfigEditor requires providerId and pageId");
  }
  configEditorStore().set(entry.providerId, {
    providerId: entry.providerId,
    activityId: entry.activityId || "",
    pageId: entry.pageId,
    label: entry.label || "打开管理界面",
    mode: entry.mode === "replace" ? "replace" : "supplement",
  });
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

function isEnabled(node, draft) {
  if (node.mutability === "read_only" || node.mutability === "computed") return false;
  if (!node.enabled_if) return true;
  try {
    return evalConfigExpr(node.enabled_if, draft);
  } catch {
    return true;
  }
}

function sortNodes(nodes) {
  return nodes
    .map((node, index) => ({ node, index }))
    .sort((a, b) => {
      const order = (a.node.presentation?.order ?? 0) - (b.node.presentation?.order ?? 0);
      return order !== 0 ? order : a.index - b.index;
    })
    .map((item) => item.node);
}

function enumOptionLabel(opt) {
  return opt.label?.default || opt.title?.default || opt.value;
}

function restartPolicyHint(node) {
  switch (node.restart_policy) {
    case "plugin_reload":
      return "更改后将重载插件";
    case "application_restart":
      return "更改后需要重启应用";
    case "host_restart":
      return "更改后需要重启宿主";
    default:
      return "";
  }
}

function isStackedField(node) {
  const kind = node.value_type?.kind;
  return kind === "object" || kind === "array" || kind === "map" || !!node.value_type?.multiline;
}

function applyEnabledState(el, enabled) {
  if (enabled || !el) return el;
  if (el.matches?.("input, select, textarea, button")) el.disabled = true;
  el.querySelectorAll?.("input, select, textarea, button").forEach((node) => {
    node.disabled = true;
  });
  return el;
}

function normalizeProviders(list) {
  if (!Array.isArray(list)) return [];
  return list.map((x) => {
    if (typeof x === "string") return x;
    return x.value || x[0] || String(x);
  });
}

function defaultContext(schema) {
  const scope = schema?.scopes?.[0] || "mutsuki.global";
  if (scope === "mutsuki.global") return { scope, qualifiers: {} };
  return { scope, qualifiers: { plugin_instance_id: "default" } };
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

function appendFieldChrome(label, node) {
  const title = document.createElement("strong");
  title.textContent = node.title?.default || node.key;
  if (node.presentation?.unit) title.textContent += ` (${node.presentation.unit})`;
  if (node.presentation?.format) {
    const fmt = document.createElement("span");
    fmt.className = "settings-row__hint";
    fmt.textContent = ` · ${node.presentation.format}`;
    title.appendChild(fmt);
  }
  label.appendChild(title);
  if (node.description?.default) {
    const help = document.createElement("div");
    help.className = "settings-row__hint";
    help.textContent = node.description.default;
    label.appendChild(help);
  }
  const restart = restartPolicyHint(node);
  if (restart) {
    const hint = document.createElement("div");
    hint.className = "settings-row__hint";
    hint.textContent = restart;
    label.appendChild(hint);
  }
}

function applyPlaceholder(el, node, fallback) {
  const placeholder = node.presentation?.placeholder || fallback;
  if (placeholder) el.placeholder = placeholder;
}

function appendSettingsRow(parent, node, draft, key, onChange, ancestorEnabled = true) {
  const row = document.createElement("div");
  row.className = "settings-row settings-row--divided";
  const stacked = isStackedField(node);
  if (stacked) row.classList.add("settings-row--stacked");
  const label = document.createElement("div");
  label.className = "settings-row__label";
  appendFieldChrome(label, node);
  const control = document.createElement("div");
  control.className = "settings-row__control";
  if (stacked) {
    control.style.width = "100%";
    control.style.alignSelf = "stretch";
  }
  control.appendChild(buildNodeInput(node, draft, key, onChange, ancestorEnabled));
  row.append(label, control);
  parent.appendChild(row);
  return row;
}

function buildNodeInput(node, draft, key, onChange, ancestorEnabled = true) {
  const kind = node.value_type?.kind;
  const format = node.presentation?.format;
  const enabled = ancestorEnabled && isEnabled(node, draft);
  const readOnly = !enabled;
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
    return applyEnabledState(host, enabled);
  }

  if (kind === "bool") {
    const input = document.createElement("input");
    input.type = "checkbox";
    input.checked = !!draft[key];
    input.disabled = readOnly;
    input.addEventListener("change", () => {
      draft[key] = input.checked;
      onChange();
    });
    return input;
  }

  if (kind === "integer" || kind === "float") {
    const input = document.createElement("input");
    input.type = "number";
    input.className = "ui-input";
    applyPlaceholder(input, node);
    input.value = draft[key] ?? node.default_value?.value ?? "";
    input.disabled = readOnly;
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
        input.disabled = readOnly;
        input.addEventListener("change", () => {
          if (input.checked) selected.add(opt.value);
          else selected.delete(opt.value);
          draft[key] = [...selected];
          onChange();
        });
        row.append(input, document.createTextNode(enumOptionLabel(opt)));
        box.appendChild(row);
      }
      return applyEnabledState(box, enabled);
    }
    const select = document.createElement("select");
    select.className = "ui-input";
    select.disabled = readOnly;
    for (const opt of options) {
      const option = document.createElement("option");
      option.value = opt.value;
      option.textContent = enumOptionLabel(opt);
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
    const configured = ["configured", "set", "keep"].includes(draft[key]?.state);
    status.textContent = configured ? "已配置" : "尚未配置";
    const input = document.createElement("input");
    input.type = "password";
    input.className = "ui-input";
    input.autocomplete = "new-password";
    applyPlaceholder(input, node, configured ? "输入新密钥以替换" : "输入密钥");
    input.disabled = readOnly;
    input.addEventListener("input", () => {
      draft[key] = input.value
        ? { state: "set", value: input.value }
        : { state: "keep" };
      onChange();
    });
    row.append(status, input);
    return applyEnabledState(row, enabled);
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
      }, enabled);
      const remove = document.createElement("button");
      remove.type = "button";
      remove.textContent = "删除";
      remove.disabled = readOnly;
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
    add.disabled = readOnly;
    add.onclick = () => {
      draft[key].push("");
      onChange();
    };
    box.appendChild(add);
    return applyEnabledState(box, enabled);
  }

  if (kind === "object") {
    if (!draft[key] || typeof draft[key] !== "object") draft[key] = {};
    const nested = document.createElement("section");
    nested.className = "card card--outlined";
    for (const child of sortNodes(node.children || [])) {
      if (!isVisible(child, draft[key])) continue;
      appendSettingsRow(nested, child, draft[key], child.key, onChange, enabled);
    }
    return applyEnabledState(nested, enabled);
  }

  if (kind === "map") {
    if (!draft[key] || typeof draft[key] !== "object") draft[key] = {};
    const box = document.createElement("div");
    box.className = "map-editor";
    for (const [mapKey, mapValue] of Object.entries(draft[key])) {
      const row = document.createElement("div");
      row.className = "map-row";
      const keyInput = document.createElement("input");
      keyInput.className = "ui-input";
      keyInput.value = mapKey;
      keyInput.placeholder = "key";
      keyInput.disabled = readOnly;
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
      }, enabled);
      const remove = document.createElement("button");
      remove.type = "button";
      remove.textContent = "删除";
      remove.disabled = readOnly;
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
    add.disabled = readOnly;
    add.onclick = () => {
      let i = 1;
      while (draft[key][`key${i}`] != null) i += 1;
      draft[key][`key${i}`] = "";
      onChange();
    };
    box.appendChild(add);
    return applyEnabledState(box, enabled);
  }

  if (kind === "file_ref" || kind === "directory_ref") {
    const input = document.createElement("input");
    input.type = "text";
    input.className = "ui-input";
    applyPlaceholder(input, node, kind === "directory_ref" ? "目录路径" : "文件路径");
    input.value = draft[key] ?? "";
    input.disabled = readOnly;
    input.addEventListener("change", () => {
      draft[key] = input.value;
      onChange();
    });
    return input;
  }

  const input = document.createElement(node.value_type?.multiline ? "textarea" : "input");
  if (input.tagName === "INPUT") input.type = "text";
  input.className = input.tagName === "TEXTAREA" ? "ui-input ui-textarea" : "ui-input";
  applyPlaceholder(input, node);
  if (input.tagName === "TEXTAREA") input.style.width = "100%";
  input.value = draft[key] ?? node.default_value?.value ?? "";
  input.disabled = readOnly;
  input.addEventListener("change", () => {
    draft[key] = input.value;
    onChange();
  });
  return input;
}

function collectFormGroups(schema) {
  const nodes = sortNodes(schema.root?.children || []);
  const declared = (schema.groups || [])
    .map((group, index) => ({ group, index }))
    .sort((a, b) => {
      const order = (a.group.order ?? 0) - (b.group.order ?? 0);
      return order !== 0 ? order : a.index - b.index;
    })
    .map((item) => item.group);
  const buckets = new Map();
  const ungrouped = [];
  for (const node of nodes) {
    const groupId = node.presentation?.group;
    if (!groupId) {
      ungrouped.push(node);
      continue;
    }
    if (!buckets.has(groupId)) buckets.set(groupId, []);
    buckets.get(groupId).push(node);
  }
  const groups = [];
  for (const group of declared) {
    groups.push({
      title: group.title?.default || group.id,
      nodes: buckets.get(group.id) || [],
    });
    buckets.delete(group.id);
  }
  for (const [id, grouped] of buckets) {
    groups.push({ title: id, nodes: grouped });
  }
  if (ungrouped.length) {
    groups.push({
      title: schema.title?.default || "基本",
      nodes: ungrouped,
    });
  }
  return groups;
}

function appendGroupCard(parent, title, nodes, draft, onChange) {
  const visible = nodes.filter((node) => isVisible(node, draft));
  if (!visible.length) return;
  const card = document.createElement("section");
  card.className = "card card--outlined";
  const heading = document.createElement("h2");
  heading.textContent = title;
  card.appendChild(heading);
  for (const node of visible) {
    appendSettingsRow(card, node, draft, node.key, onChange);
  }
  parent.appendChild(card);
}

function buildForm(schema, draft, onChange) {
  const root = document.createElement("div");
  root.className = "config-form";
  for (const group of collectFormGroups(schema)) {
    appendGroupCard(root, group.title, group.nodes, draft, onChange);
  }
  return root;
}

function appendEditorCard(parent, schema, editor) {
  const card = document.createElement("section");
  card.className = "card card--outlined";
  const titleText = schema?.title?.default;
  if (titleText) {
    const heading = document.createElement("h2");
    heading.textContent = titleText;
    card.appendChild(heading);
  }
  if (schema?.description?.default) {
    const hint = document.createElement("div");
    hint.className = "settings-row__hint";
    hint.textContent = schema.description.default;
    card.appendChild(hint);
  }
  const actions = document.createElement("div");
  actions.className = "actions";
  const button = document.createElement("button");
  button.type = "button";
  button.className = "primary";
  button.textContent = editor.label;
  button.onclick = () => {
    location.hash = editor.activityId ? `#/${editor.activityId}/${editor.pageId}` : `#/${editor.pageId}`;
  };
  actions.appendChild(button);
  card.appendChild(actions);
  parent.appendChild(card);
}

/** Embeddable config panel (no outer console shell). Used by the unified overview shell. */
export function mountConfigPanel(host, rpc, events, fixedProviderId = null) {
  const state = {
    providers: [],
    selected: null,
    schema: null,
    snapshot: null,
    draft: {},
    message: "",
    conflict: null,
    applyInFlight: false,
    pendingRevision: null,
  };

  const root = document.createElement("div");
  root.className = "config-panel settings-page";
  host.innerHTML = "";
  host.appendChild(root);

  async function refreshProviders() {
    if (fixedProviderId) {
      const schema = await rpc.call("config", "schema.get", { provider_id: fixedProviderId });
      const provider = {
        id: fixedProviderId,
        title: schema?.title?.default || "配置",
        description: schema?.description?.default || "",
        schema,
      };
      state.providers = [provider];
      await openProvider(provider);
      return;
    }
    const list = await rpc.call("config", "providers.list", {});
    const ids = normalizeProviders(list);
    state.providers = await Promise.all(ids.map(async (id) => {
      const schema = await rpc.call("config", "schema.get", { provider_id: id });
      return {
        id,
        title: schema?.title?.default || "配置",
        description: schema?.description?.default || "",
        schema,
      };
    }));
    render();
  }

  async function openProvider(provider) {
    state.selected = provider.id;
    state.conflict = null;
    state.schema = provider.schema || await rpc.call("config", "schema.get", {
      provider_id: provider.id,
    });
    const context = defaultContext(state.schema);
    state.snapshot = await rpc.call("config", "snapshot.read", {
      provider_id: provider.id,
      context,
    });
    state.draft = snapshotToDraft(state.snapshot?.value);
    render();
  }

  function render() {
    root.innerHTML = "";
    if (!state.selected) {
      const card = document.createElement("section");
      card.className = "card";
      card.innerHTML = "<h2>功能配置</h2>";
      const list = document.createElement("div");
      list.className = "provider-list";
      for (const provider of state.providers) {
        const btn = document.createElement("button");
        btn.className = "provider-item";
        btn.textContent = provider.title;
        btn.onclick = () => openProvider(provider);
        list.appendChild(btn);
      }
      if (!state.providers.length) {
        list.textContent = "暂无可配置项目";
      }
      card.appendChild(list);
      root.appendChild(card);
      return;
    }

    if (state.conflict) {
      const banner = document.createElement("div");
      banner.className = "conflict";
      banner.textContent = "配置已在其他页面更新，请重新加载后再提交。";
      const reload = document.createElement("button");
      reload.type = "button";
      reload.textContent = "重新加载";
      reload.onclick = () => {
        const selected = state.providers.find((provider) => provider.id === state.selected);
        return openProvider(selected || { id: state.selected, schema: state.schema });
      };
      banner.appendChild(reload);
      root.appendChild(banner);
    }

    const editor = configEditorStore().get(state.selected);
    if (editor) appendEditorCard(root, state.schema, editor);
    const replaceForm = editor?.mode === "replace";

    if (!replaceForm) {
      const formHost = document.createElement("div");
      const rebuild = () => {
        formHost.innerHTML = "";
        formHost.appendChild(buildForm(state.schema, state.draft, rebuild));
      };
      rebuild();
      root.appendChild(formHost);
    }

    const actions = document.createElement("div");
    actions.className = "actions";
    if (!fixedProviderId) {
      const backBtn = document.createElement("button");
      backBtn.type = "button";
      backBtn.textContent = "返回";
      backBtn.onclick = () => {
        state.selected = null;
        render();
      };
      actions.appendChild(backBtn);
    }
    if (!replaceForm) {
      const validateBtn = document.createElement("button");
      validateBtn.type = "button";
      validateBtn.textContent = "验证";
      validateBtn.onclick = async () => {
        const context = defaultContext(state.schema);
        const result = await rpc.call("config", "validate", {
          provider_id: state.selected,
          candidate: draftToCandidate(state.draft, state.schema),
          context,
        });
        state.message = result.ok ? "验证通过" : JSON.stringify(result.issues || result);
        renderMessage();
      };
      const applyBtn = document.createElement("button");
      applyBtn.type = "button";
      applyBtn.className = "primary";
      applyBtn.textContent = "应用";
      applyBtn.onclick = async () => {
        state.applyInFlight = true;
        state.pendingRevision = null;
        try {
          const context = defaultContext(state.schema);
          const result = await rpc.call("config", "apply", {
            provider_id: state.selected,
            context,
            request: {
              candidate: draftToCandidate(state.draft, state.schema),
              expected_revision: state.snapshot?.revision ?? 0,
              dry_run: false,
            },
          });
          state.conflict = null;
          const pendingActions = Array.isArray(result?.pending_actions) ? result.pending_actions : [];
          const restartRequired = pendingActions.includes("application_restart_scheduled") ||
            pendingActions.includes("host_restart_scheduled");
          state.message = restartRequired ? "配置已保存，请重启应用后继续设置。" : "配置已生效";
          const selected = state.providers.find((provider) => provider.id === state.selected);
          await openProvider(selected || { id: state.selected, schema: state.schema });
          state.applyInFlight = false;
          const pendingRevision = state.pendingRevision;
          state.pendingRevision = null;
          const currentRevision = state.snapshot?.revision;
          if (pendingRevision != null && currentRevision != null && Number(pendingRevision) !== Number(currentRevision)) {
            state.conflict = { current: pendingRevision, expected: currentRevision };
            state.message = "检测到配置已在其他页面更新";
            render();
            return;
          }
          renderMessage();
        } catch (error) {
          state.applyInFlight = false;
          state.pendingRevision = null;
          const text = String(error?.message || error);
          try {
            const parsed = JSON.parse(text);
            if (parsed.kind === "revision_conflict") {
              state.conflict = parsed;
              state.message = "配置已发生变化，请重新加载";
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
      actions.append(validateBtn, applyBtn);
    }
    if (actions.childNodes.length) root.appendChild(actions);
    const msg = document.createElement("div");
    msg.id = "message";
    msg.className = "message";
    msg.textContent = state.message;
    root.appendChild(msg);

    function renderMessage() {
      const el = root.querySelector("#message");
      if (el) el.textContent = state.message;
    }
  }

  const revisionSubscription = events?.subscribe("config.revision_changed", (payload) => {
      if (!state.selected) return;
      const provider = payload?.provider_id?.value || payload?.provider_id;
      if (provider && provider !== state.selected) return;
      const remote = payload?.revision?.value ?? payload?.revision;
      const local = state.snapshot?.revision;
      if (state.applyInFlight) {
        state.pendingRevision = remote;
        return;
      }
      if (remote != null && local != null && Number(remote) !== Number(local)) {
        state.conflict = { current: remote, expected: local };
        state.message = "检测到配置已在其他页面更新";
        render();
      }
    }, "config.schema.read");

  refreshProviders().catch(() => {
    state.message = "配置加载失败，请稍后重试";
    render();
  });
  root.destroy = () => revisionSubscription?.dispose();
  return root;
}

function createConsoleApp(rpc) {
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
        <nav class="secondary-panel__body sb-section nav" aria-label="控制台">
          <a class="sb-tree__row lilia-interactive-item" href="?page=overview"><span class="sb-tree__name">概览</span></a>
          <button type="button" data-route="config" class="sb-tree__row lilia-interactive-item is-active" aria-current="page" data-lilia-selected="true"><span class="sb-tree__name">配置</span></button>
        </nav>
        <div class="secondary-panel__footer sidebar-footer">Bot 控制台</div>
      </div>
    </aside>
    <main class="lilia-workspace-region" data-region="main">
      <div class="lilia-workspace-region__content page-scroll">
        <div class="page-header">
          <div>
            <h1>配置</h1>
            <p>管理账号、模型与回复策略</p>
          </div>
        </div>
        <section id="content" class="page-body"></section>
      </div>
    </main>
  `;
  mountConfigPanel(app.querySelector("#content"), rpc);
  return app;
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
  ensureMutsukiUiStylesheet();
  el.appendChild(createConsoleApp(rpc));
}

function encodeProviderRoute(providerId) {
  return [...new TextEncoder().encode(providerId)]
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}

export default {
  id: "config",
  async setup(ctx) {
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
    const groups = await ctx.rpc.call("config", "navigation.list", {});
    const entries = (groups || []).flatMap((group) =>
      (group.items || []).map((item) => ({ group, item })),
    );
    const providers = (await Promise.all(
      entries.map(async ({ group, item }) => {
        const schema = await ctx.rpc.call("config", "schema.get", {
          provider_id: item.provider_id,
        });
        return { group, item, schema };
      }),
    ));
    providers.forEach((provider, order) => {
      const { group, item, schema } = provider;
      const providerId = item.provider_id;
      const title = item.label || schema?.title?.default || "配置";
      const routeId = encodeProviderRoute(providerId);
      const pageId = order === 0 ? "config.page" : `config.provider.${routeId}`;
      const path = order === 0 ? "/config" : `/config/${routeId}`;
      ctx.pages.register({
        id: pageId,
        path,
        title,
        component: {
          mount(el) {
            const panel = mountConfigPanel(el, ctx.rpc, ctx.events, providerId);
            return { dispose: () => panel?.destroy?.() };
          },
        },
        requiredCapability: "config.schema.read",
      });
      ctx.navigation.register({
        id: `${pageId}.nav`,
        activityId: "config",
        pageId,
        label: title,
        group: group.label || undefined,
        order,
        requiredCapability: "config.schema.read",
      });
    });
  },
};
