const DRAFT_KEY = "mutsuki.bot-flow-editor.draft";

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

export async function mountBotFlowEditor(el, rpc) {
  el.innerHTML = `<div class="flow-editor">
    <aside class="card flow-palette"><div class="toolbar nested"><h2>流程节点</h2><button id="flow-new">新建流程</button></div><div id="flow-catalog"></div></aside>
    <main class="card flow-canvas"><div class="toolbar nested"><select id="flow-select"></select><span id="flow-state" class="pill"></span><button id="flow-validate" class="ghost">检查</button><button id="flow-apply">应用</button></div><div id="flow-issues"></div><div id="flow-nodes" class="flow-node-list"></div></main>
    <aside class="card flow-properties"><h2>属性</h2><div id="flow-properties"><p class="muted">选择节点后编辑属性。</p></div></aside>
  </div>`;
  const catalogHost = el.querySelector("#flow-catalog");
  const nodesHost = el.querySelector("#flow-nodes");
  const propertyHost = el.querySelector("#flow-properties");
  const issuesHost = el.querySelector("#flow-issues");
  const flowSelect = el.querySelector("#flow-select");
  const stateHost = el.querySelector("#flow-state");
  let catalog = [];
  let snapshot;
  let flows = [];
  let selectedNode = null;
  let pendingPort = null;
  let draggedNodeId = null;
  let draggedNodeStart = null;
  let draggedPaletteNode = null;

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
    catalogHost.innerHTML = [...groups].map(([category, nodes]) => `<section><h3>${esc(category)}</h3>${nodes.map((node) => `<button class="ghost flow-node-type" draggable="true" data-node-type="${esc(node.node_type_id)}" data-version="${node.version}">${esc(node.title)}</button>`).join("")}</section>`).join("");
    catalogHost.querySelectorAll("[data-node-type]").forEach((button) => {
      button.onclick = () => addNode(button.dataset.nodeType, Number(button.dataset.version));
      button.onpointerdown = () => {
        draggedPaletteNode = {
          node_type_id: button.dataset.nodeType,
          version: Number(button.dataset.version),
        };
      };
      button.onpointerup = () => { draggedPaletteNode = null; };
      button.ondragstart = (event) => event.dataTransfer.setData("application/x-mutsuki-node", JSON.stringify({ node_type_id: button.dataset.nodeType, version: Number(button.dataset.version) }));
    });
  }

  function addNode(type, version, position = null) {
    const flow = activeFlow();
    if (!flow) return;
    const desc = catalog.find((node) => node.node_type_id === type && node.version === version);
    const node = { node_id: newId("node"), node_type_id: type, node_type_version: version, config: {}, source: desc?.role === "source" ? { protocol_id: "mutsuki.bot.event/ingest@1", event_type: null } : null, position: position || { x: 40 + flow.nodes.length * 28, y: 40 + flow.nodes.length * 28 } };
    flow.nodes.push(node); selectedNode = node; dirty(); render();
  }

  function portType(nodeId, portId, direction) {
    const flow = activeFlow();
    const node = flow?.nodes.find((item) => item.node_id === nodeId);
    return descriptor(node)?.ports.find((port) => port.port_id === portId && port.direction === direction)?.event_type;
  }

  function sameType(left, right) {
    return left && right && left.type_id === right.type_id && left.version === right.version;
  }

  function connect(nodeId, portId, direction, kind = "event") {
    if (!pendingPort) {
      if (direction !== "output") return;
      pendingPort = { nodeId, portId, kind, eventType: kind === "error" ? { type_id: "mutsuki.bot.flow.error", version: 1 } : portType(nodeId, portId, "output") };
      render();
      return;
    }
    if (direction === "output") {
      pendingPort = { nodeId, portId, kind, eventType: kind === "error" ? { type_id: "mutsuki.bot.flow.error", version: 1 } : portType(nodeId, portId, "output") };
      render();
      return;
    }
    const targetType = portType(nodeId, portId, "input");
    if (!sameType(pendingPort.eventType, targetType)) {
      issuesHost.innerHTML = `<div class="error-banner">端口类型不兼容：${esc(pendingPort.eventType?.type_id)}@${esc(pendingPort.eventType?.version)} → ${esc(targetType?.type_id)}@${esc(targetType?.version)}</div>`;
      return;
    }
    const flow = activeFlow();
    flow.edges.push({
      edge_id: newId("edge"),
      from_node_id: pendingPort.nodeId,
      from_port_id: pendingPort.portId,
      to_node_id: nodeId,
      to_port_id: portId,
      kind: pendingPort.kind,
    });
    pendingPort = null;
    dirty();
    render();
  }

  function render() {
    flowSelect.innerHTML = flows.map((flow) => `<option value="${esc(flow.flow_id)}">${esc(flow.name)}</option>`).join("");
    const flow = activeFlow();
    if (!flow) { nodesHost.innerHTML = `<p class="muted">新建一个流程后开始编排。</p>`; return; }
    flowSelect.value = flow.flow_id;
    const edgeLines = flow.edges.map((edge) => {
      const from = flow.nodes.find((node) => node.node_id === edge.from_node_id);
      const to = flow.nodes.find((node) => node.node_id === edge.to_node_id);
      if (!from || !to) return "";
      const x1 = from.position.x + 190;
      const y1 = from.position.y + 58;
      const x2 = to.position.x;
      const y2 = to.position.y + 58;
      const curve = Math.max(50, Math.abs(x2 - x1) / 2);
      return `<path class="flow-edge ${edge.kind}" d="M ${x1} ${y1} C ${x1 + curve} ${y1}, ${x2 - curve} ${y2}, ${x2} ${y2}" data-edge="${esc(edge.edge_id)}"><title>${esc(edge.from_port_id)} → ${esc(edge.to_port_id)}</title></path>`;
    }).join("");
    nodesHost.innerHTML = `<svg class="flow-edge-layer" aria-hidden="true">${edgeLines}</svg>` + flow.nodes.map((node) => {
      const desc = descriptor(node);
      const invalid = snapshot?.validation?.issues?.some((issue) => issue.node_id === node.node_id);
      const errorPort = desc?.role !== "source" ? `<button class="flow-port output error ${pendingPort?.nodeId === node.node_id && pendingPort?.kind === "error" ? "connecting" : ""}" data-port="error" data-kind="error" title="结构化错误事件">错误</button>` : "";
      return `<article class="flow-node ${selectedNode?.node_id === node.node_id ? "selected" : ""} ${invalid ? "invalid" : ""}" draggable="true" data-node="${esc(node.node_id)}" style="transform:translate(${node.position.x}px,${node.position.y}px)"><strong>${esc(desc?.title || node.node_type_id)}</strong><div class="muted">${esc(desc?.category || "")}</div><div class="flow-ports">${(desc?.ports || []).map((port) => `<button class="flow-port ${port.direction} ${pendingPort?.nodeId === node.node_id && pendingPort?.portId === port.port_id ? "connecting" : ""}" data-port="${esc(port.port_id)}" data-direction="${esc(port.direction)}" data-kind="event" title="${esc(port.event_type.type_id)}@${port.event_type.version}">${esc(port.title)}</button>`).join("")}${errorPort}</div></article>`;
    }).join("") + `<div class="flow-edge-list"><strong>连线</strong>${flow.edges.length ? flow.edges.map((edge) => `<button class="ghost" data-delete-edge="${esc(edge.edge_id)}">${esc(edge.from_port_id)} → ${esc(edge.to_port_id)}${edge.kind === "error" ? "（错误）" : ""} ×</button>`).join("") : `<span class="muted">点击输出端口，再点击兼容的输入端口。</span>`}</div>`;
    nodesHost.querySelectorAll("[data-node]").forEach((card) => {
      card.onclick = () => { selectedNode = flow.nodes.find((node) => node.node_id === card.dataset.node); render(); };
      card.onpointerdown = (event) => {
        if (!event.target.closest("[data-port]")) {
          draggedNodeId = card.dataset.node;
          draggedNodeStart = { x: event.clientX, y: event.clientY };
        }
      };
      card.ondragstart = (event) => { draggedNodeId = card.dataset.node; event.dataTransfer.setData("application/x-mutsuki-existing-node", draggedNodeId); };
    });
    nodesHost.querySelectorAll("[data-port]").forEach((button) => button.onclick = (event) => {
      event.stopPropagation();
      connect(button.closest("[data-node]").dataset.node, button.dataset.port, button.dataset.direction || "output", button.dataset.kind);
    });
    nodesHost.querySelectorAll("[data-delete-edge]").forEach((button) => button.onclick = () => { flow.edges = flow.edges.filter((edge) => edge.edge_id !== button.dataset.deleteEdge); dirty(); render(); });
    renderProperties();
  }

  function renderProperties() {
    if (!selectedNode) { propertyHost.innerHTML = `<p class="muted">选择节点后编辑属性。</p>`; return; }
    const desc = descriptor(selectedNode);
    const properties = desc?.config_schema?.properties || {};
    const required = new Set(desc?.config_schema?.required || []);
    const fields = Object.entries(properties).map(([key, schema]) => {
      const value = selectedNode.config[key] ?? schema.default;
      const label = `${esc(schema.title || key)}${required.has(key) ? " *" : ""}`;
      if (schema.type === "boolean") return `<div class="field"><label><input data-config-key="${esc(key)}" data-config-type="boolean" type="checkbox" ${value ? "checked" : ""} /> ${label}</label></div>`;
      if (schema.type === "integer" || schema.type === "number") return `<div class="field"><label>${label}</label><input data-config-key="${esc(key)}" data-config-type="${esc(schema.type)}" type="number" value="${esc(value ?? "")}" /></div>`;
      if (schema.type === "string" && Array.isArray(schema.enum)) return `<div class="field"><label>${label}</label><select data-config-key="${esc(key)}" data-config-type="string">${schema.enum.map((option) => `<option value="${esc(option)}" ${option === value ? "selected" : ""}>${esc(option)}</option>`).join("")}</select></div>`;
      if (schema.type === "string") return `<div class="field"><label>${label}</label><input data-config-key="${esc(key)}" data-config-type="string" value="${esc(value ?? "")}" /></div>`;
      return `<div class="field"><label>${label}（JSON）</label><textarea data-config-key="${esc(key)}" data-config-type="json" rows="4">${esc(JSON.stringify(value ?? (schema.type === "array" ? [] : {}), null, 2))}</textarea></div>`;
    }).join("");
    propertyHost.innerHTML = `<div class="field"><label>节点 ID</label><input value="${esc(selectedNode.node_id)}" disabled /></div>${fields || `<p class="muted">此节点无需配置。</p>`}${desc?.role === "source" ? `<div class="field"><label>输入事件协议 *</label><input id="node-source" value="${esc(selectedNode.source?.protocol_id || "")}" /></div>` : ""}<div class="toolbar nested"><button id="node-apply">应用</button><button id="node-delete" class="ghost">删除</button></div>`;
    propertyHost.querySelector("#node-apply").onclick = () => {
      const config = {};
      propertyHost.querySelectorAll("[data-config-key]").forEach((field) => {
        let value;
        if (field.dataset.configType === "boolean") value = field.checked;
        else if (field.dataset.configType === "integer") value = Number.parseInt(field.value, 10);
        else if (field.dataset.configType === "number") value = Number(field.value);
        else if (field.dataset.configType === "json") value = JSON.parse(field.value || "null");
        else value = field.value;
        if (value !== "" && !Number.isNaN(value)) config[field.dataset.configKey] = value;
      });
      selectedNode.config = config;
      if (desc?.role === "source") selectedNode.source = { protocol_id: propertyHost.querySelector("#node-source").value.trim(), event_type: null };
      dirty(); render();
    };
    propertyHost.querySelector("#node-delete").onclick = () => { const flow = activeFlow(); flow.nodes = flow.nodes.filter((node) => node.node_id !== selectedNode.node_id); flow.edges = flow.edges.filter((edge) => edge.from_node_id !== selectedNode.node_id && edge.to_node_id !== selectedNode.node_id); selectedNode = null; dirty(); render(); };
  }

  async function validate() {
    const result = await rpc.call("bot-flow-editor", "validate", { flows });
    snapshot.validation = result;
    issuesHost.innerHTML = result.issues.length ? `<div class="error-banner">${result.issues.map((issue) => `<button class="ghost" data-issue-node="${esc(issue.node_id || "")}">${esc(issue.message)}</button>`).join("")}</div>` : `<div class="success-banner">流程校验通过</div>`;
    issuesHost.querySelectorAll("[data-issue-node]").forEach((button) => button.onclick = () => { const flow = activeFlow(); selectedNode = flow?.nodes.find((node) => node.node_id === button.dataset.issueNode) || null; render(); });
    render(); return result;
  }

  async function refresh() {
    [catalog, snapshot] = await Promise.all([
      rpc.call("bot-flow-editor", "catalog.read", {}),
      rpc.call("bot-flow-editor", "snapshot.read", {}),
    ]);
    const saved = JSON.parse(localStorage.getItem(DRAFT_KEY) || "null");
    flows = structuredClone(saved?.flows || snapshot.flows || []);
    if (saved) {
      stateHost.textContent = saved.baseRevision === snapshot.revision ? "本地草稿" : "本地草稿需要重新加载";
      stateHost.className = "pill warn";
    } else {
      stateHost.textContent = "已发布";
      stateHost.className = "pill ok";
    }
    renderCatalog(); render();
  }

  el.querySelector("#flow-new").onclick = () => { const flow = { flow_id: newId("flow"), name: `新流程 ${flows.length + 1}`, enabled: true, nodes: [], edges: [] }; flows.push(flow); flowSelect.value = flow.flow_id; dirty(); render(); };
  flowSelect.onchange = () => { selectedNode = null; render(); };
  nodesHost.ondragover = (event) => event.preventDefault();
  nodesHost.onpointerup = (event) => {
    if (!draggedPaletteNode && !draggedNodeId) return;
    const bounds = nodesHost.getBoundingClientRect();
    const position = {
      x: Math.max(0, event.clientX - bounds.left - 95),
      y: Math.max(0, event.clientY - bounds.top - 30),
    };
    if (draggedPaletteNode) {
      addNode(draggedPaletteNode.node_type_id, draggedPaletteNode.version, position);
      draggedPaletteNode = null;
      return;
    }
    const moved = draggedNodeStart
      && Math.hypot(event.clientX - draggedNodeStart.x, event.clientY - draggedNodeStart.y) > 4;
    if (!moved) {
      draggedNodeId = null;
      draggedNodeStart = null;
      return;
    }
    const node = activeFlow()?.nodes.find((item) => item.node_id === draggedNodeId);
    if (node) { node.position = position; dirty(); render(); }
    draggedNodeId = null;
    draggedNodeStart = null;
  };
  nodesHost.ondrop = (event) => {
    event.preventDefault();
    const bounds = nodesHost.getBoundingClientRect();
    const existingId = event.dataTransfer.getData("application/x-mutsuki-existing-node") || draggedNodeId;
    if (existingId) {
      const node = activeFlow()?.nodes.find((item) => item.node_id === existingId);
      if (node) { node.position = { x: Math.max(0, event.clientX - bounds.left - 95), y: Math.max(0, event.clientY - bounds.top - 30) }; dirty(); render(); }
      draggedNodeId = null;
      return;
    }
    const encoded = event.dataTransfer.getData("application/x-mutsuki-node");
    if (!encoded) return;
    const item = JSON.parse(encoded);
    addNode(item.node_type_id, item.version, { x: Math.max(0, event.clientX - bounds.left - 95), y: Math.max(0, event.clientY - bounds.top - 30) });
  };
  el.querySelector("#flow-validate").onclick = () => validate().catch((error) => { issuesHost.innerHTML = `<div class="error-banner">${esc(errorText(error))}</div>`; });
  el.querySelector("#flow-apply").onclick = async () => {
    try {
      const result = await validate();
      if (!result.valid) return;
      await rpc.call("bot-flow-editor", "apply", { expected_revision: snapshot.revision, flows });
      localStorage.removeItem(DRAFT_KEY);
      await refresh();
    } catch (error) {
      // Keep the local candidate on CAS conflict so the user can refresh deliberately.
      dirty();
      issuesHost.innerHTML = `<div class="error-banner">${esc(errorText(error))}</div>`;
    }
  };
  await refresh();
}

export default { id: "bot-flow-editor" };
