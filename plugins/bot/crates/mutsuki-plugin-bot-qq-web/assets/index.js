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
    element("p", "muted", `${account.health} · ${account.connected ? "已连接" : "未连接"}`),
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
        tableSection("会话规则", state.snapshot.conversations, [
          ["会话", (row) => row.conversation.kind + " · " + row.conversation.account_id],
          ["Agent", (row) => row.policy.agent_enabled ? "启用" : "停用"],
          ["主动投递", (row) => row.policy.active_delivery_enabled ? "启用" : "停用"],
          ["规则来源", (row) => row.matched_rule_ids.join(", ")],
        ], (row) => [
          actionButton("切换 Agent", {
            action: "conversation_update",
            conversation: row.conversation,
            policy: { ...row.policy, agent_enabled: !row.policy.agent_enabled },
          }, state, rpc, refresh),
          actionButton("切换主动投递", {
            action: "conversation_update",
            conversation: row.conversation,
            policy: { ...row.policy, active_delivery_enabled: !row.policy.active_delivery_enabled },
          }, state, rpc, refresh),
          actionButton("切换必须提及", {
            action: "conversation_update",
            conversation: row.conversation,
            policy: { ...row.policy, must_mention: !row.policy.must_mention },
          }, state, rpc, refresh),
          actionButton("切换 STT", {
            action: "conversation_update",
            conversation: row.conversation,
            policy: { ...row.policy, stt_enabled: !row.policy.stt_enabled },
          }, state, rpc, refresh),
          actionButton("切换 TTS", {
            action: "conversation_update",
            conversation: row.conversation,
            policy: { ...row.policy, tts_enabled: !row.policy.tts_enabled },
          }, state, rpc, refresh),
          actionButton("发送测试", () => {
            const message = window.prompt("输入测试消息");
            return message ? {
              action: "account_send_test",
              account_id: row.conversation.account_id,
              conversation: row.conversation,
              text: message,
            } : null;
          }, state, rpc, refresh),
        ]),
        tableSection("Handler 与命令", state.snapshot.handlers, [
          ["Handler", (row) => row.descriptor.handler_id],
          ["状态", (row) => row.enabled ? "启用" : "停用"],
          ["Generation", (row) => row.descriptor.generation],
          ["Priority", (row) => row.descriptor.priority],
          ["权限", (row) => row.descriptor.permissions.map((permission) => `${permission.scope}:${permission.action}`).join(", ")],
          ["限流", (row) => row.rate_limit_status],
          ["最近错误", (row) => row.last_error_code],
          ["Trace", (row) => row.last_invocation_trace_id],
        ], (row) => [
          actionButton(row.enabled ? "停用" : "启用", {
            action: "handler_set_enabled",
            handler_id: row.descriptor.handler_id,
            generation: row.descriptor.generation,
            enabled: !row.enabled,
          }, state, rpc, refresh),
        ]),
        tableSection("命令目录", state.snapshot.commands || [], [
          ["命令", (row) => row.path.join(" ")],
          ["Aliases", (row) => row.aliases.map((path) => path.join(" ")).join(", ")],
          ["参数", (row) => row.arguments.map((argument) => argument.name).join(", ")],
        ]),
        tableSection("Agent 会话", state.snapshot.agent_sessions, [
          ["Origin", (row) => row.binding.origin_key],
          ["Session", (row) => row.binding.session_id],
          ["状态", (row) => row.status],
          ["Approval", (row) => row.approval_status],
          ["Delivery", (row) => row.delivery_status],
          ["Version", (row) => row.binding.session_version],
        ], (row) => [
          ...(row.current_turn_id ? [actionButton("取消生成", {
            action: "agent_cancel",
            origin_key: row.binding.origin_key,
            turn_id: row.current_turn_id,
          }, state, rpc, refresh)] : []),
          actionButton("重置", {
            action: "agent_reset",
            origin_key: row.binding.origin_key,
          }, state, rpc, refresh),
          actionButton("分叉", {
            action: "agent_fork",
            origin_key: row.binding.origin_key,
          }, state, rpc, refresh),
          actionButton("重新生成", {
            action: "agent_regenerate",
            origin_key: row.binding.origin_key,
          }, state, rpc, refresh),
        ]),
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
