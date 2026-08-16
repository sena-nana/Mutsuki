import {
  formatClock,
  formatDurationMs,
  groupRecords,
} from "./trajectory-model.js";

export function mountTrajectoryView(host, options = {}) {
  const state = {
    records: [],
    groups: [],
    hasOlder: false,
    loadingOlder: false,
    loading: false,
    truncated: false,
    lost: 0,
    emptyText: "暂无时间线记录",
    onLoadOlder: null,
    onInspect: null,
    selectedId: null,
    inspectDetail: "",
    inspectError: "",
    inspectLoading: false,
    query: "",
    collapsed: new Set(),
  };

  host.classList.add("trajectory");
  host.innerHTML = `
    <div class="trajectory-toolbar">
      <button type="button" class="ghost" data-older hidden>加载更早</button>
      <label class="trajectory-search">
        <span class="visually-hidden">搜索记录</span>
        <input type="search" placeholder="搜索记录" data-query />
      </label>
      <span class="muted" data-status></span>
    </div>
    <div class="trajectory-body">
      <div class="trajectory-ledger" data-ledger></div>
      <aside class="trajectory-inspector" data-inspector>
        <div class="muted">选择一条记录查看详情</div>
      </aside>
    </div>
  `;

  const olderEl = host.querySelector("[data-older]");
  const queryEl = host.querySelector("[data-query]");
  const statusEl = host.querySelector("[data-status]");
  const ledgerEl = host.querySelector("[data-ledger]");
  const inspectorEl = host.querySelector("[data-inspector]");

  queryEl.addEventListener("input", () => {
    state.query = queryEl.value;
    render();
  });
  olderEl.addEventListener("click", async () => {
    if (!state.onLoadOlder || state.loadingOlder) return;
    await state.onLoadOlder();
  });
  ledgerEl.addEventListener("click", onLedgerClick);
  apply(options);
  render();

  return {
    update(next) {
      apply(next);
      render();
    },
    destroy() {
      host.replaceChildren();
    },
  };

  function apply(next) {
    if (Array.isArray(next.records)) state.records = next.records;
    state.groups = Array.isArray(next.groups) ? next.groups : groupRecords(state.records);
    if ("hasOlder" in next) state.hasOlder = !!next.hasOlder;
    if ("loadingOlder" in next) state.loadingOlder = !!next.loadingOlder;
    if ("loading" in next) state.loading = !!next.loading;
    if ("truncated" in next) state.truncated = !!next.truncated;
    if ("lost" in next) state.lost = Number(next.lost || 0);
    if (next.emptyText) state.emptyText = next.emptyText;
    if ("onLoadOlder" in next) state.onLoadOlder = next.onLoadOlder;
    if ("onInspect" in next) state.onInspect = next.onInspect;
    if (next.selectedId) state.selectedId = next.selectedId;
  }

  function visibleRecords() {
    const query = state.query.trim().toLowerCase();
    if (!query) return state.records;
    return state.records.filter((record) =>
      [record.label, record.preview, record.input, record.output, record.thinking, record.groupLabel]
        .filter(Boolean)
        .join("\n")
        .toLowerCase()
        .includes(query),
    );
  }

  function render() {
    const visible = visibleRecords();
    olderEl.hidden = !state.hasOlder && !state.loadingOlder;
    olderEl.disabled = state.loadingOlder || !state.onLoadOlder;
    olderEl.textContent = state.loadingOlder ? "正在加载…" : "加载更早";
    const bits = [];
    if (state.loading) bits.push("加载中");
    if (state.lost) bits.push(`丢失 ${state.lost}`);
    if (state.truncated) bits.push("已截断");
    bits.push(`${visible.length} / ${state.records.length} 条`);
    statusEl.textContent = bits.join(" · ");
    renderRows(visible);
    renderInspector(state.records.find((record) => record.id === state.selectedId) || null);
  }

  function renderRows(records) {
    if (!records.length) {
      ledgerEl.innerHTML = `<div class="muted trajectory-empty">${esc(state.loading ? "加载中…" : state.emptyText)}</div>`;
      return;
    }
    const byGroup = new Map();
    for (const record of records) {
      const bucket = byGroup.get(record.groupId) || [];
      bucket.push(record);
      byGroup.set(record.groupId, bucket);
    }
    ledgerEl.innerHTML = state.groups
      .map((group) => {
        const bucket = byGroup.get(group.id);
        if (!bucket?.length) return "";
        const closed = state.collapsed.has(group.id);
        const rows = closed
          ? ""
          : bucket.map((record) => renderRow(record)).join("");
        return `<button type="button" class="trajectory-group" data-group="${esc(group.id)}"><span>${closed ? "▸" : "▾"} ${esc(group.label)}</span><span class="muted">${bucket.length}</span></button>${rows}`;
      })
      .join("");
  }

  function renderRow(record) {
    const selected = record.id === state.selectedId ? " is-selected" : "";
    const err = record.isError ? " is-error" : "";
    const running = record.status === "running" ? " is-running" : "";
    const nest = record.kind === "subtool" || record.parentId ? " is-nested" : "";
    const time =
      record.durationMs != null
        ? formatDurationMs(record.durationMs)
        : record.status === "running"
          ? "进行中"
          : "—";
    return `<button type="button" class="trajectory-row is-${esc(record.kind)}${selected}${err}${running}${nest}" data-record="${esc(record.id)}"><span class="trajectory-row__index">#${record.index}</span><span class="trajectory-row__kind">${esc(record.label)}</span><span class="trajectory-row__preview">${esc(record.preview || "—")}</span><span class="trajectory-row__time">${esc(time)}</span></button>`;
  }

  function renderInspector(record) {
    if (!record) {
      inspectorEl.innerHTML = `<div class="muted">选择一条记录查看详情</div>`;
      return;
    }
    const jsonBody = state.inspectLoading
      ? "正在加载完整事件…"
      : state.inspectError || state.inspectDetail;
    inspectorEl.innerHTML = `
      <h3>${esc(record.label)} <span class="muted">#${record.index}</span></h3>
      <ul class="kv">
        <li><span>状态</span><span>${esc(statusLabel(record.status))}</span></li>
        <li><span>开始</span><span>${esc(formatClock(record.startedAt))}</span></li>
        <li><span>耗时</span><span>${esc(formatDurationMs(record.durationMs))}</span></li>
        <li><span>事件序号</span><span>${esc((record.sequences || []).join(", ") || "—")}</span></li>
      </ul>
      ${block("Input", record.input)}
      ${block("Output", record.output)}
      ${block("Thinking", record.thinking)}
      ${block("JSON", jsonBody)}
    `;
  }

  function requestInspect(record) {
    state.inspectDetail = "";
    state.inspectError = "";
    if (!record || !state.onInspect) {
      state.inspectLoading = false;
      render();
      return;
    }
    state.inspectLoading = true;
    render();
    const selected = record.id;
    Promise.resolve(state.onInspect(record))
      .then((detail) => {
        if (state.selectedId !== selected) return;
        state.inspectLoading = false;
        state.inspectDetail =
          detail == null || detail === ""
            ? ""
            : typeof detail === "string"
              ? detail
              : JSON.stringify(detail, null, 2);
        render();
      })
      .catch((error) => {
        if (state.selectedId !== selected) return;
        state.inspectLoading = false;
        state.inspectError = String(error?.message || error || "加载完整事件失败");
        render();
      });
  }

  function onLedgerClick(event) {
    const group = event.target.closest("[data-group]");
    if (group) {
      const id = group.dataset.group;
      if (state.collapsed.has(id)) state.collapsed.delete(id);
      else state.collapsed.add(id);
      render();
      return;
    }
    const row = event.target.closest("[data-record]");
    if (!row) return;
    state.selectedId = row.dataset.record;
    requestInspect(state.records.find((record) => record.id === state.selectedId) || null);
  }
}

function statusLabel(status) {
  if (status === "running") return "进行中";
  if (status === "error") return "失败";
  return "完成";
}

function block(title, value) {
  if (value == null || value === "") return "";
  return `<h4>${esc(title)}</h4><pre class="log-block">${esc(value)}</pre>`;
}

function esc(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}
