const STYLE = `
.database-layout { display: grid; grid-template-columns: 240px minmax(0,1fr); gap: 12px; min-height: 560px; }
.database-col { display: flex; flex-direction: column; min-height: 0; min-width: 0; }
.database-tables { overflow: auto; flex: 1; display: flex; flex-direction: column; gap: 6px; }
.database-item { border: 1px solid var(--border, transparent); border-radius: 10px; padding: 10px 12px; background: var(--bg-elev, transparent); text-align: left; cursor: pointer; }
.database-item.is-active { outline: 2px solid var(--accent, #7aa2ff); }
.database-grid { overflow: auto; flex: 1; }
.database-grid .data-table { min-width: max-content; }
.database-grid .data-table th { white-space: nowrap; }
.database-grid .data-table td { max-width: 280px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.database-cell--null { color: var(--muted, #888); font-style: italic; }
.database-pager { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
@media (max-width: 960px) { .database-layout { grid-template-columns: 1fr; min-height: 0; } }
`;

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;");
}

function formatSize(bytes) {
  const size = Number(bytes);
  if (!Number.isFinite(size) || size < 0) return "—";
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
  return `${(size / (1024 * 1024)).toFixed(1)} MB`;
}

function formatCell(value) {
  if (value == null) return { text: "NULL", kind: "null" };
  if (typeof value === "object" && value.$type === "blob") {
    return { text: `<blob ${Number(value.bytes) || 0} bytes>`, kind: "blob" };
  }
  if (typeof value === "object") return { text: JSON.stringify(value), kind: "json" };
  return { text: String(value), kind: "text" };
}

function errorMessage(error, fallback = "操作失败，请稍后重试") {
  const raw = error?.message || String(error ?? "");
  return raw.startsWith("extension ") || raw.includes("rpc ") ? fallback : raw || fallback;
}

function mountDatabase(host, rpc) {
  host.className = "page-body";
  host.innerHTML = `
    <style>${STYLE}</style>
    <div class="database-panel settings-page stack">
      <div class="toolbar row-item">
        <button type="button" class="ghost" data-refresh>刷新</button>
        <span class="muted" data-status>加载中…</span>
      </div>
      <div data-body></div>
    </div>
  `;
  const body = host.querySelector("[data-body]");
  const status = host.querySelector("[data-status]");
  const state = { snapshot: null, selected: "", page: null, cursors: [""], pageIndex: 0 };
  let disposed = false;

  const setStatus = (message, isError = false) => {
    if (disposed) return;
    status.className = isError ? "err-text" : "muted";
    status.textContent = message;
  };

  const render = () => {
    const snapshot = state.snapshot;
    if (!snapshot) {
      body.innerHTML = `<p class="muted">当前没有已接入的数据库</p>`;
      return;
    }
    const tables = snapshot.tables || [];
    const selected = tables.find((table) => table.name === state.selected);
    const rows = state.page?.rows || [];
    const columns = state.page?.columns || [];
    body.innerHTML = `
      <div class="database-layout">
        <section class="card card--outlined database-col">
          <h2>数据表</h2>
          <div class="database-tables">
            ${
              tables.length
                ? tables
                    .map(
                      (table) => `
              <button type="button" class="database-item${table.name === state.selected ? " is-active" : ""}" data-table="${escapeHtml(table.name)}">
                <strong>${escapeHtml(table.name)}</strong>
                <p class="muted">${table.row_count ?? 0} 行 · ${(table.columns || []).length} 列</p>
              </button>`,
                    )
                    .join("")
                : `<p class="muted">没有可查看的数据表</p>`
            }
          </div>
        </section>
        <section class="card card--outlined database-col">
          <h2>${escapeHtml(state.selected || "数据库")}</h2>
          <p class="muted">${escapeHtml(["sqlite", snapshot.journal_mode, formatSize(snapshot.size_bytes), snapshot.path].filter(Boolean).join(" · "))}</p>
          ${
            !state.selected
              ? `<p class="muted">在左侧选择要访问的数据表</p>`
              : !state.page
                ? `<p class="muted">加载中…</p>`
                : `
            <div class="database-grid">
              <table class="data-table">
                <thead><tr>${columns
                  .map((column) => {
                    const field = (selected?.columns || []).find((item) => item.name === column);
                    return `<th title="${escapeHtml(field?.decl_type || "")}">${escapeHtml(field?.primary_key ? `${column} *` : column)}</th>`;
                  })
                  .join("")}</tr></thead>
                <tbody>
                  ${
                    rows.length
                      ? rows
                          .map(
                            (row) => `<tr>${columns
                              .map((_, index) => {
                                const cell = formatCell(row[index]);
                                return `<td class="database-cell--${cell.kind}" title="${escapeHtml(cell.text)}">${escapeHtml(cell.text)}</td>`;
                              })
                              .join("")}</tr>`,
                          )
                          .join("")
                      : `<tr><td class="muted" colspan="${Math.max(columns.length, 1)}">此表暂无数据</td></tr>`
                  }
                </tbody>
              </table>
            </div>
            <div class="database-pager">
              <button type="button" class="ghost" data-prev ${state.pageIndex <= 0 ? "disabled" : ""}>上一页</button>
              <button type="button" class="ghost" data-next ${state.page?.next_cursor ? "" : "disabled"}>下一页</button>
              <span class="muted">第 ${state.pageIndex + 1} 页 · ${rows.length} 行</span>
            </div>`
          }
        </section>
      </div>
    `;
    body.querySelectorAll("[data-table]").forEach((item) => {
      item.onclick = () => {
        const name = item.getAttribute("data-table");
        if (!name || state.selected === name) return;
        state.selected = name;
        state.cursors = [""];
        state.pageIndex = 0;
        void refreshRows();
      };
    });
    const prev = body.querySelector("[data-prev]");
    if (prev) {
      prev.onclick = () => {
        if (state.pageIndex <= 0) return;
        state.pageIndex -= 1;
        void refreshRows();
      };
    }
    const next = body.querySelector("[data-next]");
    if (next) {
      next.onclick = () => {
        if (!state.page?.next_cursor) return;
        state.cursors[state.pageIndex + 1] = state.page.next_cursor;
        state.pageIndex += 1;
        void refreshRows();
      };
    }
  };

  const loadRows = async () => {
    if (!state.selected) {
      state.page = null;
      return;
    }
    state.page = await rpc.read("database", "rows", {
      table: state.selected,
      after: state.cursors[state.pageIndex] || "",
      limit: 50,
    });
  };

  const refreshRows = async () => {
    try {
      await loadRows();
      render();
      setStatus("");
    } catch (error) {
      setStatus(errorMessage(error, "读取数据表失败"), true);
    }
  };

  const refresh = async () => {
    if (disposed) return;
    try {
      state.snapshot = await rpc.read("database", "snapshot");
      const tables = state.snapshot?.tables || [];
      if (state.selected && !tables.some((table) => table.name === state.selected)) {
        state.selected = "";
        state.page = null;
        state.cursors = [""];
        state.pageIndex = 0;
      }
      if (state.selected) await loadRows();
      render();
      setStatus("");
    } catch (error) {
      if (!disposed) setStatus(errorMessage(error, "更新失败，请稍后重试"), true);
    }
  };

  host.querySelector("[data-refresh]").onclick = () => void refresh();
  void refresh();
  return {
    dispose() {
      disposed = true;
    },
  };
}

export default {
  id: "database",
  setup(ctx) {
    ctx.navigation.register({
      id: "database.nav",
      activityId: "system",
      pageId: "database.page",
      label: "数据库",
      order: 20,
      requiredCapability: "runtime.read",
    });
    ctx.pages.register({
      id: "database.page",
      path: "/database",
      title: "数据库",
      component: { mount: (element) => mountDatabase(element, ctx.rpc) },
      requiredCapability: "runtime.read",
    });
  },
};
