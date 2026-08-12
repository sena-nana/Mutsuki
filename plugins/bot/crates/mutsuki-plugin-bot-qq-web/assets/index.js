const text = (value) => String(value ?? "—");

const labels = {
  ok: "正常",
  degraded: "连接中",
  unhealthy: "异常",
  stopped: "未启动",
  reconnecting: "正在重连",
  disconnected: "未连接",
  connected: "已连接",
  identified: "已上线",
  resumable: "等待恢复",
  configured: "已配置",
  absent: "未配置",
  restricted: "仅可写入",
  ready: "可用",
  pending: "等待投递",
  delivered: "已送达",
  retry_scheduled: "等待重试",
  permanently_failed: "投递失败",
  reconcile_required: "需要处理",
  cancelled: "已取消",
  waiting: "等待处理",
  completed: "已完成",
  expired: "已过期",
};

const productLabel = (value) => labels[value] || text(value);

function element(tag, className, content) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (content !== undefined) node.textContent = content;
  return node;
}

function errorMessage(error, fallback = "操作失败，请稍后重试") {
  const raw = error?.message || String(error ?? "");
  const start = raw.indexOf("{");
  if (start >= 0) {
    try {
      const parsed = JSON.parse(raw.slice(start));
      if (typeof parsed.message === "string" && parsed.message.trim()) return parsed.message;
    } catch (_) {}
  }
  return raw.startsWith("extension ") || raw.includes("rpc ") ? fallback : raw || fallback;
}

function actionButton(label, action, state, rpc, refresh, options = {}) {
  const button = element("button", "ghost", label);
  button.type = "button";
  button.onclick = async () => {
    if (options.confirm !== false && !window.confirm(`确认${label}？`)) return;
    const resolved = typeof action === "function" ? action() : action;
    if (!resolved) return;
    const operationId = button.dataset.operationId || crypto.randomUUID();
    button.dataset.operationId = operationId;
    button.disabled = true;
    try {
      const result = await rpc.write("qq-bot", "write", {
        confirmed: options.confirm !== false,
        request: {
          operation_id: operationId,
          expected_revision: state.snapshot.revision,
          action: resolved,
        },
      });
      delete button.dataset.operationId;
      await refresh();
      options.onResult?.(result);
    } catch (error) {
      state.reportError?.(error);
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
    element("p", "muted", `${productLabel(account.health)} · ${productLabel(account.connection_state)}`),
    element("p", "", `心跳 ${text(account.last_heartbeat_unix_ms)} · 分片 ${account.shard.join("/")}`),
    element("p", "", `凭据 ${productLabel(account.credential_status)} · 发送状态 ${productLabel(account.rate_limit_status)}`),
  );
  const actions = element("div", "actions");
  actions.append(
    actionButton("健康检查", { action: "account_health_check", account_id: account.account_id }, state, rpc, refresh),
    actionButton("重新连接", { action: "account_reconnect", account_id: account.account_id }, state, rpc, refresh),
  );
  card.append(actions);
  if (!account.capability?.active_message) return card;
  const sendForm = element("div", "toolbar nested");
  const scene = element("select");
  const activeKinds = new Set(account.capability?.active_message_kinds || []);
  [["private", "私聊"], ["group", "群聊"], ["channel", "频道"]]
    .filter(([value]) => activeKinds.has(value))
    .forEach(([value, label]) => {
    const option = element("option", "", label);
    option.value = value;
    scene.append(option);
  });
  if (!scene.options.length) return card;
  const target = element("input");
  target.placeholder = "用户 OpenID";
  const channel = element("input");
  channel.placeholder = "频道 ID";
  channel.hidden = true;
  const message = element("input");
  message.placeholder = "测试消息";
  const sendResult = element("span", "muted");
  scene.onchange = () => {
    target.placeholder = scene.value === "private" ? "用户 OpenID" : scene.value === "group" ? "群 OpenID" : "频道组 ID";
    channel.hidden = scene.value !== "channel";
  };
  const send = actionButton("发送测试", () => {
    const id = target.value.trim();
    const textValue = message.value.trim();
    if (!id || !textValue || (scene.value === "channel" && !channel.value.trim())) {
      sendResult.textContent = "请填写接收目标和消息";
      return null;
    }
    const conversation = { version: 1, account_id: account.account_id, kind: scene.value };
    if (scene.value === "private") conversation.user_id = id;
    if (scene.value === "group") conversation.group_id = id;
    if (scene.value === "channel") {
      conversation.guild_id = id;
      conversation.channel_id = channel.value.trim();
    }
    return { action: "account_send_test", account_id: account.account_id, conversation, text: textValue };
  }, state, rpc, async () => {
    sendResult.textContent = "发送成功";
    await refresh();
  });
  sendForm.append(scene, target, channel, message, send, sendResult);
  card.append(sendForm);
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

function withLoadMore(section, cursor, load) {
  if (!cursor) return section;
  const footer = element("div", "actions");
  const button = element("button", "ghost", "加载更多");
  button.type = "button";
  button.onclick = async () => {
    button.disabled = true;
    try {
      await load(cursor);
    } finally {
      button.disabled = false;
    }
  };
  footer.append(button);
  section.append(footer);
  return section;
}

/** Mount the QQ operations panel into the shared console shell. */
export function mountQqBotPanel(host, rpc, options = {}) {
  const state = {
    snapshot: null,
    deliveries: [],
    interactions: [],
    deliveryCursor: null,
    interactionCursor: null,
    actorId: options.actorId || "web-console",
  };
  host.innerHTML = "";
  const root = element("div", "qq-bot-panel stack");
  const search = element("input", "");
  search.type = "search";
  search.placeholder = "搜索账号、会话或投递";
  const status = element("div", "muted", "正在加载…");
  root.append(search, status);
  host.append(root);

  state.reportError = (error) => {
    status.className = "error-banner";
    status.textContent = errorMessage(error);
  };

  function render() {
    root.querySelectorAll("section, article").forEach((node) => node.remove());
    state.snapshot.accounts.forEach((account) => root.append(accountCard(account, state, rpc, refresh)));
    const deliveries = tableSection("主动投递", state.deliveries, [
      ["投递记录", (row) => row.receipt.delivery_id],
      ["状态", (row) => productLabel(row.receipt.status)],
      ["尝试次数", (row) => row.receipt.attempt_count],
    ], (row) => {
      const buttons = [actionButton("预览", {
        action: "delivery_preview",
        delivery_id: row.receipt.delivery_id,
      }, state, rpc, refresh, {
        confirm: false,
        onResult: (result) => {
          const attemptCount = result?.result?.attempt_count;
          status.className = "muted";
          status.textContent = `投递预览：已尝试 ${attemptCount ?? row.receipt.attempt_count} 次，未改变投递状态`;
        },
      })];
      if (["retry_scheduled", "permanently_failed", "reconcile_required"].includes(row.receipt.status)) buttons.push(actionButton("重试", { action: "delivery_retry", delivery_id: row.receipt.delivery_id }, state, rpc, refresh));
      if (["retry_scheduled", "reconcile_required"].includes(row.receipt.status)) buttons.push(actionButton("取消", { action: "delivery_cancel", delivery_id: row.receipt.delivery_id }, state, rpc, refresh));
      return buttons;
    });
    root.append(withLoadMore(deliveries, state.deliveryCursor, loadMoreDeliveries));

    const interactions = tableSection("交互会话", state.interactions, [
      ["会话", (row) => row.session_id],
      ["状态", (row) => productLabel(row.status)],
      ["版本", (row) => row.version],
    ], (row) => row.status === "waiting" ? [actionButton("取消", {
      action: "interaction_cancel",
      session_id: row.session_id,
    }, state, rpc, refresh)] : []);
    root.append(withLoadMore(interactions, state.interactionCursor, loadMoreInteractions));
  }

  async function loadMoreDeliveries(after) {
    try {
      const page = await rpc.read("qq-bot", "deliveries.list", { query: search.value, after, limit: 50 });
      state.deliveries.push(...page.items);
      state.deliveryCursor = page.next_cursor;
      render();
    } catch (error) {
      state.reportError(error);
    }
  }

  async function loadMoreInteractions(after) {
    try {
      const page = await rpc.read("qq-bot", "interactions.list", { query: search.value, after, limit: 50 });
      state.interactions.push(...page.items);
      state.interactionCursor = page.next_cursor;
      render();
    } catch (error) {
      state.reportError(error);
    }
  }

  async function refresh() {
    try {
      const [snapshot, deliveries, interactions] = await Promise.all([
        rpc.read("qq-bot", "snapshot", { query: search.value }),
        rpc.read("qq-bot", "deliveries.list", { query: search.value, limit: 50 }),
        rpc.read("qq-bot", "interactions.list", { query: search.value, limit: 50 }),
      ]);
      state.snapshot = snapshot;
      state.deliveries = deliveries.items;
      state.deliveryCursor = deliveries.next_cursor;
      state.interactions = interactions.items;
      state.interactionCursor = interactions.next_cursor;
      status.className = "muted";
      status.textContent = "运营数据已更新";
      render();
    } catch (error) {
      state.reportError(error);
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
