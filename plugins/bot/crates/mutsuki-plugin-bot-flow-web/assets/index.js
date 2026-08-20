import { mountLiliaNodeEditor, samePortType } from "./lilia-node-editor.js";

const DRAFT_KEY = "mutsuki.bot-flow-editor.draft";
const ERROR_TYPE = { type_id: "mutsuki.bot.flow.error", version: 1 };

function esc(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function errorText(error) {
  if (error && typeof error === "object" && error.message) return error.message;
  return String(error ?? "操作失败");
}

function newId(prefix) {
  return `${prefix}-${crypto.randomUUID()}`;
}

function schemaTitle(schema, key) {
  return schema?.title || key;
}

function enumLabels(schema) {
  const values = schema?.enum || schema?.items?.enum || [];
  const titles = schema?.enumTitles || schema?.items?.enumTitles || [];
  return values.map((value, index) => ({ value, label: titles[index] || value }));
}

function defaultConfig(desc) {
  const config = {};
  for (const [key, schema] of Object.entries(desc?.config_schema?.properties || {})) {
    if (schema.default !== undefined) config[key] = structuredClone(schema.default);
    else if (schema.type === "array") config[key] = [];
    else if (schema.type === "boolean") config[key] = false;
  }
  return config;
}

function sourceSelector(desc) {
  const eventType = desc?.ports?.find((port) => port.direction === "output")?.event_type;
  return {
    protocol_id: "mutsuki.bot.event/ingest@1",
    event_type: eventType ? { type_id: eventType.type_id, version: eventType.version } : null,
  };
}

export async function mountBotFlowEditor(el, rpc) {
  el.innerHTML = `<div class="lilia-node-editor lilia-workspace" data-lilia-surface-mode="solid" data-agent-id="bot-flow-editor">
    <aside class="lilia-workspace-region lilia-node-editor__palette" data-region="start">
      <div class="lilia-workspace-region__content lilia-node-editor__pane">
        <div class="toolbar nested">
          <h2 class="lilia-node-editor__pane-title">节点</h2>
          <button type="button" id="flow-new">新建流程</button>
        </div>
        <div id="flow-catalog" class="lilia-node-editor__catalog"></div>
      </div>
    </aside>
    <section class="lilia-workspace-region lilia-node-editor__stage" data-region="main">
      <div class="lilia-workspace-region__content lilia-node-editor__stage-body">
        <div class="lilia-node-editor__toolbar">
          <select id="flow-select" aria-label="流程"></select>
          <span id="flow-state" class="pill"></span>
          <button type="button" id="flow-validate" class="ghost">检查</button>
          <button type="button" id="flow-apply" class="primary">应用</button>
        </div>
        <div id="flow-issues" class="lilia-node-editor__issues"></div>
        <div id="flow-graph"></div>
      </div>
    </section>
    <aside class="lilia-workspace-region lilia-node-editor__inspector" data-region="end">
      <div class="lilia-workspace-region__content lilia-node-editor__pane">
        <h2 class="lilia-node-editor__pane-title">属性</h2>
        <div id="flow-properties"><p class="muted">选择节点后编辑属性。</p></div>
      </div>
    </aside>
  </div>`;
  const catalogHost = el.querySelector("#flow-catalog");
  const propertyHost = el.querySelector("#flow-properties");
  const issuesHost = el.querySelector("#flow-issues");
  const flowSelect = el.querySelector("#flow-select");
  const stateHost = el.querySelector("#flow-state");
  const graph = mountLiliaNodeEditor(el.querySelector("#flow-graph"), {
    onSelectNode(nodeId) {
      const flow = activeFlow();
      selectedNode = flow?.nodes.find((node) => node.node_id === nodeId) || null;
      selectedEdgeId = null;
      graph.setSelection({ nodeId, edgeId: null });
      renderProperties();
    },
    onSelectEdge(edgeId) {
      selectedEdgeId = edgeId;
      selectedNode = null;
      graph.setSelection({ nodeId: null, edgeId });
      renderProperties();
    },
    onSelectBackground() {
      selectedNode = null;
      selectedEdgeId = null;
      graph.setSelection({ nodeId: null, edgeId: null });
      renderProperties();
    },
    onMove(nodeId, x, y) {
      const node = activeFlow()?.nodes.find((item) => item.node_id === nodeId);
      if (!node) return;
      node.position = { x, y };
      dirty();
    },
    onConnect(edge) {
      connectPorts(edge.from, edge.fromPort, edge.to, edge.toPort, edge.kind);
    },
    onInvalidConnect() {
      issuesHost.innerHTML = `<div class="error-banner">这两个端口不能连接。</div>`;
    },
    onDropPalette(item, position) {
      addNode(item.node_type_id, item.version, position);
    },
  });
  let catalog = [];
  let snapshot;
  let flows = [];
  let selectedNode = null;
  let selectedEdgeId = null;

  function activeFlow() { return flows.find((flow) => flow.flow_id === flowSelect.value) || flows[0]; }
  function descriptor(node) { return catalog.find((item) => item.node_type_id === node.node_type_id && item.version === node.node_type_version); }
  function dirty() {
    localStorage.setItem(DRAFT_KEY, JSON.stringify({ baseRevision: snapshot.revision, flows }));
    stateHost.textContent = "本地草稿";
    stateHost.className = "pill warn";
  }

  function renderCatalog() {
    const groups = new Map();
    catalog.forEach((node) => {
      const category = node.category || "其他";
      if (!groups.has(category)) groups.set(category, []);
      groups.get(category).push(node);
    });
    catalogHost.innerHTML = [...groups].map(([category, nodes]) => `<section><h3>${esc(category)}</h3>${nodes.map((node) => `<button type="button" class="ghost lilia-node-editor__type" draggable="true" data-node-type="${esc(node.node_type_id)}" data-version="${node.version}">${esc(node.title)}</button>`).join("")}</section>`).join("");
    catalogHost.querySelectorAll("[data-node-type]").forEach((button) => {
      button.onclick = () => addNode(button.dataset.nodeType, Number(button.dataset.version));
      button.ondragstart = (event) => event.dataTransfer.setData("application/x-mutsuki-node", JSON.stringify({ node_type_id: button.dataset.nodeType, version: Number(button.dataset.version) }));
    });
  }

  function addNode(type, version, position = null) {
    const flow = activeFlow();
    if (!flow) return;
    const desc = catalog.find((node) => node.node_type_id === type && node.version === version);
    const node = {
      node_id: newId("node"),
      node_type_id: type,
      node_type_version: version,
      config: defaultConfig(desc),
      source: desc?.role === "source" ? sourceSelector(desc) : null,
      position: position || { x: 48 + flow.nodes.length * 28, y: 48 + flow.nodes.length * 28 },
    };
    flow.nodes.push(node);
    selectedNode = node;
    selectedEdgeId = null;
    dirty();
    render();
  }

  function portType(nodeId, portId, direction) {
    const flow = activeFlow();
    const node = flow?.nodes.find((item) => item.node_id === nodeId);
    return descriptor(node)?.ports.find((port) => port.port_id === portId && port.direction === direction)?.event_type;
  }

  function portTitle(flow, nodeId, portId, direction) {
    if (portId === "error") return "错误";
    const node = flow?.nodes.find((item) => item.node_id === nodeId);
    return descriptor(node)?.ports.find((port) => port.port_id === portId && port.direction === direction)?.title || "";
  }

  function connectPorts(fromNodeId, fromPortId, toNodeId, toPortId, kind = "event") {
    const sourceType = kind === "error" ? ERROR_TYPE : portType(fromNodeId, fromPortId, "output");
    const targetType = portType(toNodeId, toPortId, "input");
    if (!samePortType(sourceType, targetType)) {
      issuesHost.innerHTML = `<div class="error-banner">这两个端口不能连接。</div>`;
      return;
    }
    const flow = activeFlow();
    const exists = flow.edges.some((edge) =>
      edge.from_node_id === fromNodeId &&
      edge.from_port_id === fromPortId &&
      edge.to_node_id === toNodeId &&
      edge.to_port_id === toPortId,
    );
    if (exists) return;
    flow.edges.push({
      edge_id: newId("edge"),
      from_node_id: fromNodeId,
      from_port_id: fromPortId,
      to_node_id: toNodeId,
      to_port_id: toPortId,
      kind,
    });
    dirty();
    render();
  }

  function graphModel(flow) {
    if (!flow) return { nodes: [], edges: [], emptyText: "新建一个流程后开始编排。" };
    return {
      emptyText: flow.nodes.length ? "" : "从左侧拖入节点，再从输出端口拖到兼容的输入端口。",
      nodes: flow.nodes.map((node) => {
        const desc = descriptor(node);
        const ports = desc?.ports || [];
        const outputs = ports.filter((port) => port.direction === "output").map((port) => ({
          port_id: port.port_id,
          title: port.title,
          kind: "event",
          event_type: port.event_type,
        }));
        if (desc?.role !== "source") {
          outputs.push({ port_id: "error", title: "错误", kind: "error", event_type: ERROR_TYPE });
        }
        return {
          id: node.node_id,
          x: node.position?.x ?? 40,
          y: node.position?.y ?? 40,
          title: desc?.title || node.node_type_id,
          subtitle: desc?.category || "",
          selected: selectedNode?.node_id === node.node_id,
          invalid: snapshot?.validation?.issues?.some((issue) => issue.node_id === node.node_id),
          inputs: ports.filter((port) => port.direction === "input").map((port) => ({
            port_id: port.port_id,
            title: port.title,
            kind: "event",
            event_type: port.event_type,
          })),
          outputs,
        };
      }),
      edges: flow.edges.map((edge) => ({
        id: edge.edge_id,
        from: edge.from_node_id,
        fromPort: edge.from_port_id,
        to: edge.to_node_id,
        toPort: edge.to_port_id,
        kind: edge.kind,
        selected: selectedEdgeId === edge.edge_id,
        label: edge.kind === "error"
          ? "错误"
          : [portTitle(flow, edge.from_node_id, edge.from_port_id, "output"), portTitle(flow, edge.to_node_id, edge.to_port_id, "input")].filter(Boolean).join(" → "),
      })),
    };
  }

  function renderGraph() {
    graph.setGraph(graphModel(activeFlow()));
  }

  function render() {
    flowSelect.innerHTML = flows.map((flow) => `<option value="${esc(flow.flow_id)}">${esc(flow.name)}</option>`).join("");
    const flow = activeFlow();
    if (flow) flowSelect.value = flow.flow_id;
    renderGraph();
    renderProperties();
  }

  function renderProperties() {
    const flow = activeFlow();
    if (selectedEdgeId && flow) {
      const edge = flow.edges.find((item) => item.edge_id === selectedEdgeId);
      if (!edge) {
        selectedEdgeId = null;
      } else {
        propertyHost.innerHTML = `<p class="muted">${edge.kind === "error" ? "错误连线" : "事件连线"}</p><div class="toolbar nested"><button type="button" id="edge-delete" class="ghost danger">删除连线</button></div>`;
        propertyHost.querySelector("#edge-delete").onclick = () => {
          flow.edges = flow.edges.filter((item) => item.edge_id !== selectedEdgeId);
          selectedEdgeId = null;
          dirty();
          render();
        };
        return;
      }
    }
    if (!selectedNode) {
      propertyHost.innerHTML = `<p class="muted">选择节点或连线后编辑属性。</p>`;
      return;
    }
    const desc = descriptor(selectedNode);
    const properties = desc?.config_schema?.properties || {};
    const required = new Set(desc?.config_schema?.required || []);
    const fields = Object.entries(properties).map(([key, schema]) => renderConfigField(key, schema, selectedNode.config[key], required.has(key))).join("");
    propertyHost.innerHTML = `${desc?.title ? `<p class="lilia-node-editor__hint">${esc(desc.title)}</p>` : ""}${fields || `<p class="muted">此节点无需配置。</p>`}<div class="toolbar nested"><button type="button" id="node-apply">应用</button><button type="button" id="node-delete" class="ghost danger">删除</button></div>`;
    propertyHost.querySelector("#node-apply").onclick = () => {
      selectedNode.config = readConfigFields(propertyHost, properties);
      if (desc?.role === "source") selectedNode.source = sourceSelector(desc);
      dirty();
      render();
    };
    propertyHost.querySelector("#node-delete").onclick = () => {
      const current = activeFlow();
      current.nodes = current.nodes.filter((node) => node.node_id !== selectedNode.node_id);
      current.edges = current.edges.filter((edge) => edge.from_node_id !== selectedNode.node_id && edge.to_node_id !== selectedNode.node_id);
      selectedNode = null;
      dirty();
      render();
    };
    propertyHost.querySelectorAll("[data-list-add]").forEach((button) => {
      button.onclick = () => {
        const key = button.dataset.listAdd;
        const schema = properties[key];
        const input = propertyHost.querySelector(`[data-list-input="${CSS.escape(key)}"]`);
        const value = input?.value?.trim();
        if (!value) return;
        const current = Array.isArray(selectedNode.config[key]) ? selectedNode.config[key] : [];
        if (schema?.items?.type === "array") current.push(value.split(/\s+/).filter(Boolean));
        else if (schema?.items?.type === "object") current.push({ name: value, kind: "string", optional: false, variadic: false });
        else current.push(value);
        selectedNode.config[key] = current;
        dirty();
        renderProperties();
      };
    });
    propertyHost.querySelectorAll("[data-list-remove]").forEach((button) => {
      button.onclick = () => {
        const key = button.dataset.listRemove;
        const index = Number(button.dataset.index);
        selectedNode.config[key] = (selectedNode.config[key] || []).filter((_, item) => item !== index);
        dirty();
        renderProperties();
      };
    });
  }

  function renderConfigField(key, schema, value, required) {
    const label = `${esc(schemaTitle(schema, key))}${required ? " *" : ""}`;
    const hint = schema.description ? `<div class="field-help">${esc(schema.description)}</div>` : "";
    if (schema.type === "boolean") {
      return `<div class="field"><label><input data-config-key="${esc(key)}" data-config-type="boolean" type="checkbox" ${value ? "checked" : ""} /> ${label}</label>${hint}</div>`;
    }
    if (schema.type === "integer" || schema.type === "number") {
      return `<div class="field"><label>${label}</label><input data-config-key="${esc(key)}" data-config-type="${esc(schema.type)}" type="number" value="${esc(value ?? schema.default ?? "")}" />${hint}</div>`;
    }
    if (schema.type === "string" && Array.isArray(schema.enum)) {
      return `<div class="field"><label>${label}</label><select data-config-key="${esc(key)}" data-config-type="string">${enumLabels(schema).map((option) => `<option value="${esc(option.value)}" ${option.value === (value ?? schema.default) ? "selected" : ""}>${esc(option.label)}</option>`).join("")}</select>${hint}</div>`;
    }
    if (schema.type === "string") {
      return `<div class="field"><label>${label}</label><input data-config-key="${esc(key)}" data-config-type="string" value="${esc(value ?? "")}" />${hint}</div>`;
    }
    if (schema.type === "array" && Array.isArray(schema.items?.enum)) {
      const selected = new Set(Array.isArray(value) ? value : []);
      return `<div class="field"><label>${label}</label><div class="enum-multi">${enumLabels(schema).map((option) => `<label><input data-config-key="${esc(key)}" data-config-type="enum-multi" type="checkbox" value="${esc(option.value)}" ${selected.has(option.value) ? "checked" : ""} /> ${esc(option.label)}</label>`).join("")}</div>${hint}</div>`;
    }
    if (schema.type === "array" && schema.items?.type === "string") {
      const items = Array.isArray(value) ? value : [];
      return `<div class="field"><label>${label}</label>${items.map((item, index) => `<div class="array-row"><input value="${esc(item)}" disabled /><button type="button" class="ghost" data-list-remove="${esc(key)}" data-index="${index}">删除</button></div>`).join("")}<div class="array-row"><input data-list-input="${esc(key)}" /><button type="button" data-list-add="${esc(key)}">添加</button></div>${hint}</div>`;
    }
    if (schema.type === "array" && schema.items?.type === "array") {
      const items = Array.isArray(value) ? value.map((item) => Array.isArray(item) ? item.join(" ") : String(item ?? "")) : [];
      return `<div class="field"><label>${label}</label>${items.map((item, index) => `<div class="array-row"><input value="${esc(item)}" disabled /><button type="button" class="ghost" data-list-remove="${esc(key)}" data-index="${index}">删除</button></div>`).join("")}<div class="array-row"><input data-list-input="${esc(key)}" placeholder="用空格分隔" /><button type="button" data-list-add="${esc(key)}">添加</button></div>${hint}</div>`;
    }
    if (schema.type === "array" && schema.items?.type === "object") {
      const items = Array.isArray(value) ? value : [];
      return `<div class="field"><label>${label}</label>${items.map((item, index) => `<div class="array-row"><span>${esc(item.name || `参数 ${index + 1}`)}</span><button type="button" class="ghost" data-list-remove="${esc(key)}" data-index="${index}">删除</button></div>`).join("")}<div class="array-row"><input data-list-input="${esc(key)}" placeholder="参数名" /><button type="button" data-list-add="${esc(key)}">添加</button></div>${hint}</div>`;
    }
    return "";
  }

  function readConfigFields(host, properties) {
    const config = {};
    for (const [key, schema] of Object.entries(properties || {})) {
      if (schema.type === "array" && Array.isArray(schema.items?.enum)) {
        config[key] = [...host.querySelectorAll(`[data-config-key="${CSS.escape(key)}"]`)]
          .filter((field) => field.checked)
          .map((field) => field.value);
        continue;
      }
      if (schema.type === "array" && schema.items?.type === "string") {
        config[key] = Array.isArray(selectedNode.config[key]) ? selectedNode.config[key] : [];
        continue;
      }
      if (schema.type === "array" && schema.items?.type === "array") {
        config[key] = (Array.isArray(selectedNode.config[key]) ? selectedNode.config[key] : [])
          .map((item) => Array.isArray(item) ? item : String(item).split(/\s+/).filter(Boolean));
        continue;
      }
      if (schema.type === "array" && schema.items?.type === "object") {
        config[key] = Array.isArray(selectedNode.config[key]) ? selectedNode.config[key] : [];
        continue;
      }
      const field = host.querySelector(`[data-config-key="${CSS.escape(key)}"]`);
      if (!field) continue;
      if (field.dataset.configType === "boolean") config[key] = field.checked;
      else if (field.dataset.configType === "integer") config[key] = Number.parseInt(field.value, 10);
      else if (field.dataset.configType === "number") config[key] = Number(field.value);
      else config[key] = field.value;
    }
    return config;
  }

  async function validate() {
    const result = await rpc.call("bot-flow-editor", "validate", { flows });
    snapshot.validation = result;
    issuesHost.innerHTML = result.issues.length
      ? `<div class="error-banner">${result.issues.map((issue) => `<button type="button" class="ghost" data-issue-node="${esc(issue.node_id || "")}">${esc(issue.message)}</button>`).join("")}</div>`
      : `<div class="success-banner">流程校验通过</div>`;
    issuesHost.querySelectorAll("[data-issue-node]").forEach((button) => button.onclick = () => {
      const flow = activeFlow();
      selectedNode = flow?.nodes.find((node) => node.node_id === button.dataset.issueNode) || null;
      selectedEdgeId = null;
      render();
    });
    render();
    return result;
  }

  async function refresh() {
    const [catalogBody, snapshotBody] = await Promise.all([
      rpc.call("bot-flow-editor", "catalog.read", {}),
      rpc.call("bot-flow-editor", "snapshot.read", {}),
    ]);
    catalog = Array.isArray(catalogBody) ? catalogBody : catalogBody?.nodes || [];
    snapshot = snapshotBody;
    const saved = JSON.parse(localStorage.getItem(DRAFT_KEY) || "null");
    flows = structuredClone(saved?.flows || snapshot.flows || []);
    if (saved) {
      stateHost.textContent = saved.baseRevision === snapshot.revision ? "本地草稿" : "本地草稿需要重新加载";
      stateHost.className = "pill warn";
    } else {
      stateHost.textContent = "已发布";
      stateHost.className = "pill ok";
    }
    renderCatalog();
    render();
  }

  el.querySelector("#flow-new").onclick = () => {
    const flow = { flow_id: newId("flow"), name: `新流程 ${flows.length + 1}`, enabled: true, nodes: [], edges: [] };
    flows.push(flow);
    flowSelect.value = flow.flow_id;
    selectedNode = null;
    selectedEdgeId = null;
    dirty();
    render();
  };
  flowSelect.onchange = () => {
    selectedNode = null;
    selectedEdgeId = null;
    render();
  };
  el.querySelector("#flow-validate").onclick = () => validate().catch((error) => {
    issuesHost.innerHTML = `<div class="error-banner">${esc(errorText(error))}</div>`;
  });
  el.querySelector("#flow-apply").onclick = async () => {
    try {
      const result = await validate();
      if (!result.valid) return;
      await rpc.call("bot-flow-editor", "apply", { expected_revision: snapshot.revision, flows });
      localStorage.removeItem(DRAFT_KEY);
      await refresh();
    } catch (error) {
      dirty();
      issuesHost.innerHTML = `<div class="error-banner">${esc(errorText(error))}</div>`;
    }
  };
  const onKey = (event) => {
    if (!el.contains(document.activeElement) && !el.contains(event.target)) return;
    if (event.key === "Escape") graph.cancelDraft();
    if ((event.key === "Backspace" || event.key === "Delete") && !["INPUT", "TEXTAREA", "SELECT"].includes(event.target.tagName)) {
      const flow = activeFlow();
      if (!flow) return;
      if (selectedEdgeId) {
        flow.edges = flow.edges.filter((edge) => edge.edge_id !== selectedEdgeId);
        selectedEdgeId = null;
        dirty();
        render();
      } else if (selectedNode) {
        flow.nodes = flow.nodes.filter((node) => node.node_id !== selectedNode.node_id);
        flow.edges = flow.edges.filter((edge) => edge.from_node_id !== selectedNode.node_id && edge.to_node_id !== selectedNode.node_id);
        selectedNode = null;
        dirty();
        render();
      }
    }
  };
  document.addEventListener("keydown", onKey);
  await refresh();
  return {
    dispose() {
      document.removeEventListener("keydown", onKey);
      graph.dispose();
    },
  };
}

export default {
  id: "bot-flow-editor",
  setup(ctx) {
    ctx.pages.register({
      id: "bot-flow.page", path: "/flows", title: "流程编排",
      component: { mount: (el) => mountBotFlowEditor(el, ctx.rpc) },
      requiredCapability: "bot.flow.read",
    });
    ctx.navigation.register({
      id: "bot-flow.nav", activityId: "automation", pageId: "bot-flow.page", label: "流程编排", order: 10,
      requiredCapability: "bot.flow.read",
    });
  },
};
