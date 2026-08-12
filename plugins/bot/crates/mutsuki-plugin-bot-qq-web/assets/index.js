const text = (value) => String(value ?? "—");

function element(tag, className, content) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (content !== undefined) node.textContent = content;
  return node;
}

function actionButton(label, action, state, rpc, refresh, options = {}) {
  const button = element("button", "ghost", label);
  button.type = "button";
  button.onclick = async () => {
    if (options.confirm !== false && !window.confirm(`确认${label}？`)) return;
    const resolved = typeof action === "function" ? action() : action;
    if (!resolved) return;
    button.disabled = true;
    try {
      await rpc.write("qq-bot", "write", {
        confirmed: options.confirm !== false,
        request: {
          actor_id: state.actorId,
          expected_revision: state.snapshot.revision,
          action: resolved,
        },
      });
      await refresh();
    } finally {
      button.disabled = false;
    }
  };
  return button;
}

function accountCard(account, state, rpc, refresh) {
  const card = element("article", "panel nested");
  card.append(
    element("h3", "", account.account_id),
    element("p", "muted", `${account.health} · ${account.connection_state === "disconnected" ? "未连接" : "已连接"}`),
    element("p", "", `心跳 ${text(account.last_heartbeat_unix_ms)} · 分片 ${account.shard.join("/")}`),
    element("p", "", `凭据 ${text(account.credential_status)} · 限流 ${text(account.rate_limit_status)}`),
  );
  const actions = element("div", "actions");
  actions.append(
    actionButton(account.enabled ? "停用账号" : "启用账号", {
      action: "account_set_enabled",
      account_id: account.account_id,
      enabled: !account.enabled,
    }, state, rpc, refresh),
    actionButton("健康检查", { action: "account_health_check", account_id: account.account_id }, state, rpc, refresh),
    actionButton("重新连接", { action: "account_reconnect", account_id: account.account_id }, state, rpc, refresh),
  );
  card.append(actions);
  return card;
}

function tableSection(title, rows, columns, actions) {
  const section = element("section", "panel");
  section.append(element("h2", "", title));
  if (!rows.length) {
    section.append(element("p", "muted", "暂无记录"));
    return section;
  }
  const table = element("table", "data-table");
  const head = element("tr");
  columns.forEach(([label]) => head.append(element("th", "", label)));
  if (actions) head.append(element("th", "", "操作"));
  table.append(head);
  rows.forEach((row) => {
    const tr = element("tr");
    columns.forEach(([, value]) => tr.append(element("td", "", text(value(row)))));
    if (actions) {
      const cell = element("td", "actions");
      const buttons = actions(row);
      if (buttons) cell.append(...buttons);
      tr.append(cell);
    }
    table.append(tr);
  });
  section.append(table);
  return section;
}

/** Mount the QQ operations panel into the shared console shell. */
export function mountQqBotPanel(host, rpc, options = {}) {
  const state = { snapshot: null, actorId: options.actorId || "web-console" };
  host.innerHTML = "";
  const root = element("div", "qq-bot-panel stack");
  const search = element("input", "");
  search.type = "search";
  search.placeholder = "搜索账号、会话或投递";
  const status = element("div", "muted", "正在加载…");
  root.append(search, status);
  host.append(root);

  async function refresh() {
    try {
      state.snapshot = await rpc.read("qq-bot", "snapshot", { query: search.value });
      root.querySelectorAll("section, article").forEach((node) => node.remove());
      status.textContent = `Revision ${state.snapshot.revision}`;
      state.snapshot.accounts.forEach((account) => root.append(accountCard(account, state, rpc, refresh)));
      root.append(
        tableSection("主动投递", state.snapshot.deliveries, [
          ["Delivery", (row) => row.receipt.delivery_id],
          ["状态", (row) => row.receipt.status],
          ["Attempts", (row) => row.receipt.attempt_count],
        ], (row) => [
          actionButton("预览", {
            action: "delivery_preview",
            delivery_id: row.receipt.delivery_id,
          }, state, rpc, refresh, { confirm: false }),
          actionButton("重试", {
            action: "delivery_retry",
            delivery_id: row.receipt.delivery_id,
          }, state, rpc, refresh),
          actionButton("取消", {
            action: "delivery_cancel",
            delivery_id: row.receipt.delivery_id,
          }, state, rpc, refresh),
        ]),
        tableSection("交互会话", state.snapshot.interactions, [
          ["Session", (row) => row.session_id],
          ["状态", (row) => row.status],
          ["Version", (row) => row.version],
        ], (row) => row.status === "waiting" ? [actionButton("取消", {
          action: "interaction_cancel",
          session_id: row.session_id,
        }, state, rpc, refresh)] : []),
      );
    } catch (error) {
      status.className = "error-banner";
      status.textContent = error?.message || String(error);
    }
  }

  let timer;
  search.oninput = () => {
    clearTimeout(timer);
    timer = setTimeout(refresh, 180);
  };
  refresh();
  return { refresh, destroy: () => clearTimeout(timer) };
}
