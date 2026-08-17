export const NODE_WIDTH = 220;
export const NODE_HEADER = 66;
export const PORT_ROW = 28;

export function edgePath(x1, y1, x2, y2) {
  const curve = Math.max(48, Math.abs(x2 - x1) / 2);
  return `M ${x1} ${y1} C ${x1 + curve} ${y1}, ${x2 - curve} ${y2}, ${x2} ${y2}`;
}

export function samePortType(left, right) {
  return Boolean(
    left &&
      right &&
      left.type_id === right.type_id &&
      Number(left.version) === Number(right.version),
  );
}

export function portAnchor(node, port, index) {
  const y = node.y + NODE_HEADER + index * PORT_ROW;
  const x = port.direction === "output" ? node.x + NODE_WIDTH : node.x;
  return { x, y };
}

export function clientToWorld(bounds, view, clientX, clientY) {
  return {
    x: (clientX - bounds.left - view.x) / view.scale,
    y: (clientY - bounds.top - view.y) / view.scale,
  };
}

function esc(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function applyWorldTransform(world, view) {
  world.style.transform = `translate(${view.x}px, ${view.y}px) scale(${view.scale})`;
}

function portIndex(ports, portId) {
  return Math.max(0, ports.findIndex((port) => port.port_id === portId));
}

export function mountLiliaNodeEditor(host, handlers = {}) {
  host.classList.add("lilia-node-editor__viewport");
  host.innerHTML = `
    <div class="lilia-node-editor__world">
      <svg class="lilia-node-editor__wires" aria-hidden="true"></svg>
      <div class="lilia-node-editor__nodes"></div>
    </div>
    <div class="lilia-node-editor__empty muted" hidden></div>
  `;
  const world = host.querySelector(".lilia-node-editor__world");
  const wires = host.querySelector(".lilia-node-editor__wires");
  const nodesHost = host.querySelector(".lilia-node-editor__nodes");
  const empty = host.querySelector(".lilia-node-editor__empty");
  const view = { x: 24, y: 24, scale: 1 };
  let graph = { nodes: [], edges: [] };
  let draft = null;
  let drag = null;
  let pan = null;

  function worldPoint(event) {
    return clientToWorld(host.getBoundingClientRect(), view, event.clientX, event.clientY);
  }

  function nodeById(nodeId) {
    return graph.nodes.find((node) => node.id === nodeId);
  }

  function updateGrid() {
    host.style.setProperty("--lilia-node-editor-grid-x", `${view.x}px`);
    host.style.setProperty("--lilia-node-editor-grid-y", `${view.y}px`);
    applyWorldTransform(world, view);
  }

  function measuredPort(nodeId, portId, direction) {
    const button = nodesHost.querySelector(
      `[data-node="${CSS.escape(nodeId)}"] [data-port="${CSS.escape(portId)}"][data-direction="${direction}"]`,
    );
    const dot = button?.querySelector(".lilia-node-editor__port-dot") || button;
    if (!dot) return null;
    const rect = dot.getBoundingClientRect();
    const bounds = host.getBoundingClientRect();
    return {
      x: (rect.left + rect.width / 2 - bounds.left - view.x) / view.scale,
      y: (rect.top + rect.height / 2 - bounds.top - view.y) / view.scale,
    };
  }

  function wireGeometry(edge) {
    const from = nodeById(edge.from);
    const to = nodeById(edge.to);
    if (!from || !to) return "";
    const start = measuredPort(edge.from, edge.fromPort, "output")
      || portAnchor(from, { direction: "output" }, portIndex(from.outputs, edge.fromPort));
    const end = measuredPort(edge.to, edge.toPort, "input")
      || portAnchor(to, { direction: "input" }, portIndex(to.inputs, edge.toPort));
    return edgePath(start.x, start.y, end.x, end.y);
  }

  function renderWires() {
    const draftPath = draft
      ? `<path class="lilia-node-editor__wire is-draft" d="${esc(edgePath(draft.x1, draft.y1, draft.x2, draft.y2))}"></path>`
      : "";
    wires.innerHTML = graph.edges.map((edge) => {
      const d = wireGeometry(edge);
      if (!d) return "";
      return `<path class="lilia-node-editor__wire ${edge.kind === "error" ? "error" : ""} ${edge.selected ? "is-selected" : ""}" data-edge="${esc(edge.id)}" d="${esc(d)}"><title>${esc(edge.label || "")}</title></path>`;
    }).join("") + draftPath;
    wires.querySelectorAll("[data-edge]").forEach((path) => {
      path.style.pointerEvents = "stroke";
      path.addEventListener("pointerdown", (event) => {
        event.stopPropagation();
        handlers.onSelectEdge?.(path.dataset.edge);
      });
    });
  }

  function renderNodes() {
    empty.hidden = graph.nodes.length > 0;
    empty.textContent = graph.emptyText || "";
    nodesHost.innerHTML = graph.nodes.map((node) => {
      const inputPorts = node.inputs.map((port, index) => portMarkup(node, port, "input", index)).join("");
      const outputPorts = node.outputs.map((port, index) => portMarkup(node, port, "output", index)).join("");
      return `<article class="lilia-node-editor__node${node.selected ? " is-selected" : ""}${node.invalid ? " is-invalid" : ""}" data-node="${esc(node.id)}" style="transform:translate(${node.x}px,${node.y}px)">
        <h3 class="lilia-node-editor__node-title">${esc(node.title)}</h3>
        <p class="lilia-node-editor__node-meta">${esc(node.subtitle || "")}</p>
        <div class="lilia-node-editor__ports">
          <div class="lilia-node-editor__port-col lilia-node-editor__port-col--input">${inputPorts}</div>
          <div class="lilia-node-editor__port-col lilia-node-editor__port-col--output">${outputPorts}</div>
        </div>
      </article>`;
    }).join("");
    nodesHost.querySelectorAll("[data-node]").forEach((card) => {
      card.addEventListener("pointerdown", (event) => {
        if (event.target.closest("[data-port]")) return;
        event.stopPropagation();
        const node = nodeById(card.dataset.node);
        if (!node) return;
        handlers.onSelectNode?.(node.id);
        const live = nodesHost.querySelector(`[data-node="${CSS.escape(node.id)}"]`) || card;
        const point = worldPoint(event);
        const current = nodeById(node.id) || node;
        drag = {
          id: node.id,
          offsetX: point.x - current.x,
          offsetY: point.y - current.y,
          pointerId: event.pointerId,
        };
        live.classList.add("is-dragging", "is-selected");
        live.setPointerCapture?.(event.pointerId);
      });
    });
    nodesHost.querySelectorAll("[data-port]").forEach((button) => {
      button.addEventListener("pointerdown", (event) => {
        event.stopPropagation();
        const node = nodeById(button.closest("[data-node]").dataset.node);
        const direction = button.dataset.direction;
        const portId = button.dataset.port;
        const kind = button.dataset.kind || "event";
        if (!node) return;
        if (direction === "output") {
          const start = measuredPort(node.id, portId, "output")
            || portAnchor(node, { direction: "output" }, portIndex(node.outputs, portId));
          draft = {
            from: node.id,
            fromPort: portId,
            kind,
            eventType: portType(node, portId, "output", kind),
            x1: start.x,
            y1: start.y,
            x2: start.x,
            y2: start.y,
            pointerId: event.pointerId,
          };
          host.classList.add("is-wiring");
          button.classList.add("is-active");
          button.setPointerCapture?.(event.pointerId);
          renderWires();
          return;
        }
        if (draft) {
          finishConnect(node.id, portId, direction, kind);
        }
      });
      button.addEventListener("pointerenter", () => {
        if (draft && button.dataset.direction === "input") button.classList.add("is-target");
      });
      button.addEventListener("pointerleave", () => button.classList.remove("is-target"));
    });
  }

  function portType(node, portId, direction, kind) {
    if (kind === "error") return { type_id: "mutsuki.bot.flow.error", version: 1 };
    const ports = direction === "output" ? node.outputs : node.inputs;
    return ports.find((port) => port.port_id === portId)?.event_type;
  }

  function portMarkup(node, port, direction, index) {
    const active = draft?.from === node.id && draft?.fromPort === port.port_id;
    return `<button type="button" class="lilia-node-editor__port ${direction}${port.kind === "error" ? " error" : ""}${active ? " is-active" : ""}" data-port="${esc(port.port_id)}" data-direction="${direction}" data-kind="${esc(port.kind || "event")}" data-index="${index}" title="${esc(port.title)}">
      <span class="lilia-node-editor__port-dot"></span>
      <span class="lilia-node-editor__port-label">${esc(port.title)}</span>
    </button>`;
  }

  function finishConnect(nodeId, portId, direction, kind) {
    if (!draft || direction !== "input") return false;
    const target = nodeById(nodeId);
    if (!target || draft.from === nodeId) {
      cancelDraft();
      return false;
    }
    const targetType = portType(target, portId, "input", kind);
    if (!samePortType(draft.eventType, targetType)) {
      handlers.onInvalidConnect?.(draft.eventType, targetType);
      cancelDraft();
      return false;
    }
    const payload = {
      from: draft.from,
      fromPort: draft.fromPort,
      to: nodeId,
      toPort: portId,
      kind: draft.kind,
    };
    cancelDraft();
    handlers.onConnect?.(payload);
    return true;
  }

  function cancelDraft() {
    draft = null;
    host.classList.remove("is-wiring");
    host.querySelectorAll(".is-active, .is-target").forEach((el) => {
      el.classList.remove("is-active", "is-target");
    });
    renderWires();
  }

  function onPointerMove(event) {
    if (pan) {
      view.x = pan.viewX + (event.clientX - pan.x);
      view.y = pan.viewY + (event.clientY - pan.y);
      updateGrid();
      return;
    }
    if (drag) {
      const point = worldPoint(event);
      const node = nodeById(drag.id);
      if (!node) return;
      node.x = Math.max(0, point.x - drag.offsetX);
      node.y = Math.max(0, point.y - drag.offsetY);
      const card = nodesHost.querySelector(`[data-node="${CSS.escape(drag.id)}"]`);
      if (card) card.style.transform = `translate(${node.x}px, ${node.y}px)`;
      renderWires();
      return;
    }
    if (draft) {
      const point = worldPoint(event);
      draft.x2 = point.x;
      draft.y2 = point.y;
      renderWires();
    }
  }

  function onPointerUp(event) {
    if (pan && pan.pointerId === event.pointerId) {
      pan = null;
      host.classList.remove("is-panning");
      return;
    }
    if (drag && drag.pointerId === event.pointerId) {
      const node = nodeById(drag.id);
      drag = null;
      host.querySelector(".is-dragging")?.classList.remove("is-dragging");
      if (node) handlers.onMove?.(node.id, node.x, node.y);
      return;
    }
    if (draft && draft.pointerId === event.pointerId) {
      const target = document.elementFromPoint(event.clientX, event.clientY)?.closest("[data-port]");
      if (target) {
        finishConnect(
          target.closest("[data-node]").dataset.node,
          target.dataset.port,
          target.dataset.direction,
          target.dataset.kind,
        );
        return;
      }
      if (Math.hypot(draft.x2 - draft.x1, draft.y2 - draft.y1) > 8) cancelDraft();
    }
  }

  host.addEventListener("pointerdown", (event) => {
    if (event.target.closest("[data-node], [data-edge]")) return;
    if (event.button === 1 || event.button === 0) {
      pan = { x: event.clientX, y: event.clientY, viewX: view.x, viewY: view.y, pointerId: event.pointerId };
      host.classList.add("is-panning");
      host.setPointerCapture?.(event.pointerId);
      handlers.onSelectBackground?.();
    }
  });
  host.addEventListener("pointermove", onPointerMove);
  host.addEventListener("pointerup", onPointerUp);
  host.addEventListener("pointercancel", onPointerUp);
  host.addEventListener("wheel", (event) => {
    if (!event.ctrlKey && !event.metaKey) return;
    event.preventDefault();
    const point = worldPoint(event);
    const next = Math.min(1.8, Math.max(0.55, view.scale * (event.deltaY < 0 ? 1.08 : 0.92)));
    view.x = event.clientX - host.getBoundingClientRect().left - point.x * next;
    view.y = event.clientY - host.getBoundingClientRect().top - point.y * next;
    view.scale = next;
    updateGrid();
  }, { passive: false });
  host.addEventListener("dragover", (event) => event.preventDefault());
  host.addEventListener("drop", (event) => {
    event.preventDefault();
    const encoded = event.dataTransfer?.getData("application/x-mutsuki-node");
    if (!encoded) return;
    const item = JSON.parse(encoded);
    const point = worldPoint(event);
    handlers.onDropPalette?.(item, { x: Math.max(0, point.x - NODE_WIDTH / 2), y: Math.max(0, point.y - 24) });
  });

  updateGrid();
  return {
    setGraph(next) {
      graph = {
        nodes: (next.nodes || []).map((node) => ({ ...node })),
        edges: next.edges || [],
        emptyText: next.emptyText || "",
      };
      renderNodes();
      renderWires();
    },
    setSelection({ nodeId = null, edgeId = null } = {}) {
      for (const node of graph.nodes) node.selected = node.id === nodeId;
      for (const edge of graph.edges) edge.selected = edge.id === edgeId;
      nodesHost.querySelectorAll("[data-node]").forEach((card) => {
        card.classList.toggle("is-selected", card.dataset.node === nodeId);
      });
      renderWires();
    },
    clientToWorld(clientX, clientY) {
      return clientToWorld(host.getBoundingClientRect(), view, clientX, clientY);
    },
    cancelDraft,
    dispose() {
      host.replaceChildren();
    },
  };
}
