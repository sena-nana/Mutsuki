const QQ_PROVIDER_ID = "mutsuki.bot.adapter.qqbot";

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
  keep: "已保存",
};

const productLabel = (value) => labels[value] || text(value);

function element(tag, className, content) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (content !== undefined) node.textContent = content;
  return node;
}

function parsedError(error) {
  const raw = error?.message || String(error ?? "");
  const start = raw.indexOf("{");
  if (start >= 0) {
    try {
      const parsed = JSON.parse(raw.slice(start));
      return {
        code: parsed.code || parsed.kind || "",
        message: typeof parsed.message === "string" ? parsed.message : raw,
      };
    } catch (_) {}
  }
  return { code: "", message: raw };
}

function errorMessage(error, fallback = "操作失败，请稍后重试") {
  const { message } = parsedError(error);
  return message.startsWith("extension ") || message.includes("rpc ") ? fallback : message || fallback;
}

function isOwnerUnavailable(error) {
  const { code, message } = parsedError(error);
  return code === "qq.owner_unavailable" || message.includes("尚未连接") || message.includes("尚未启用");
}

function formatTime(unixMs) {
  const value = Number(unixMs);
  if (!unixMs || Number.isNaN(value) || value <= 0) return "—";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "—";
  return date.toLocaleString();
}

function goPage(page) {
  const routes = {
    config: "/config/config.page",
    "bot-flow": "/automation/bot-flow.page",
    "agent-connections": "/bot/bot-agent.page",
  };
  if (routes[page]) {
    location.hash = routes[page];
    return;
  }
  const url = new URL(location.href);
  if (page === "overview") url.searchParams.delete("page");
  else url.searchParams.set("page", page);
  url.searchParams.delete("tab");
  history.pushState({}, "", url);
  window.dispatchEvent(new PopStateEvent("popstate"));
}

function wireToPlain(value) {
  if (!value || typeof value !== "object") return value;
  if (["bool", "integer", "float", "string"].includes(value.type)) return value.value;
  if (value.type === "secret") return value.value;
  if (value.type === "array") return (value.value || []).map(wireToPlain);
  if (value.type === "object") {
    const out = {};
    for (const [key, child] of Object.entries(value.value || {})) out[key] = wireToPlain(child);
    return out;
  }
  if (value.state) return value;
  if (!("type" in value)) {
    const out = {};
    for (const [key, child] of Object.entries(value)) out[key] = wireToPlain(child);
    return out;
  }
  return value;
}

function snapshotDraft(snapshot) {
  return wireToPlain(snapshot?.value) || {};
}

async function optionalCall(rpc, namespace, method, params) {
  try {
    return { ok: true, value: await rpc.call(namespace, method, params || {}) };
  } catch (error) {
    return { ok: false, error };
  }
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
          expected_revision: state.snapshot?.revision ?? 0,
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
  const card = element("article", "card card--outlined");
  card.dataset.accountId = account.account_id;
  card.append(
    element("h3", "", account.account_id || "QQ 账号"),
    element("p", "muted", `${productLabel(account.health)} · ${productLabel(account.connection_state)}`),
    element("p", "", `App ID ${text(account.app_id)} · 分片 ${account.shard.join("/")}`),
    element("p", "", `心跳 ${formatTime(account.last_heartbeat_unix_ms)} · 重连 ${text(account.reconnect_count)} 次`),
    element("p", "", `凭据 ${productLabel(account.credential_status)} · 发送状态 ${productLabel(account.rate_limit_status)}`),
  );
  if (account.last_error) card.append(element("p", "error-banner", account.last_error));
  const actions = element("div", "actions");
  actions.append(
    actionButton("健康检查", { action: "account_health_check", account_id: account.account_id }, state, rpc, refresh),
    actionButton("重新连接", { action: "account_reconnect", account_id: account.account_id }, state, rpc, refresh),
  );
  card.append(actions);
  if (!account.capability?.active_message) return card;
  const sendForm = element("div", "toolbar nested");
  const scene = element("select", "ui-input");
  scene.dataset.draftField = "scene";
  const activeKinds = new Set(account.capability?.active_message_kinds || []);
  [["private", "私聊"], ["group", "群聊"], ["channel", "频道"]]
    .filter(([value]) => activeKinds.has(value))
    .forEach(([value, label]) => {
    const option = element("option", "", label);
    option.value = value;
    scene.append(option);
  });
  if (!scene.options.length) return card;
  const target = element("input", "ui-input");
  target.dataset.draftField = "target";
  target.placeholder = "用户 OpenID";
  const channel = element("input", "ui-input");
  channel.dataset.draftField = "channel";
  channel.placeholder = "频道 ID";
  channel.hidden = true;
  const message = element("input", "ui-input");
  message.dataset.draftField = "message";
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
  const section = element("section", "card card--outlined");
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

function linkButton(label, page) {
  const button = element("button", "ghost", label);
  button.type = "button";
  button.onclick = () => goPage(page);
  return button;
}

function settingsRow(label, hint, control) {
  const row = element("div", "settings-row settings-row--divided");
  const title = element("div", "settings-row__label");
  title.append(element("strong", "", label));
  if (hint) title.append(element("div", "settings-row__hint", hint));
  const host = element("div", "settings-row__control");
  host.append(control);
  row.append(title, host);
  return row;
}

function loginCard(state, refresh) {
  const section = element("section", "card card--outlined");
  section.append(element("h2", "", "QQ 登录"));
  if (state.ownerUnavailable) {
    section.append(element("p", "muted", "尚未启用 QQ Bot。填写 App ID 与 Client Secret 并启用后才会连接。"));
  }
  if (!state.configAvailable) {
    section.append(element("p", "muted", "登录配置不可用。"));
    return section;
  }
  const draft = state.loginDraft || {};
  const secretConfigured = draft.client_secret?.state === "configured" || draft.client_secret?.state === "keep";
  const enabled = Object.assign(element("input"), { type: "checkbox", checked: !!draft.enabled });
  const appId = Object.assign(element("input", "ui-input"), { type: "text", value: draft.app_id || "" });
  appId.placeholder = "开放平台 App ID";
  const secret = Object.assign(element("input", "ui-input"), { type: "password" });
  secret.placeholder = secretConfigured ? "已保存，留空则不更改" : "Client Secret";
  secret.autocomplete = "new-password";
  const privateGroup = Object.assign(element("input"), { type: "checkbox", checked: draft.receive_private_and_group !== false });
  const guild = Object.assign(element("input"), { type: "checkbox", checked: !!draft.receive_guild });
  const apply = element("button", "primary", "保存登录配置");
  apply.type = "button";
  apply.onclick = async () => {
    apply.disabled = true;
    try {
      await state.rpc.call("config", "apply", {
        provider_id: QQ_PROVIDER_ID,
        context: { scope: "mutsuki.global" },
        request: {
          candidate: {
            enabled: enabled.checked,
            app_id: appId.value.trim(),
            client_secret: secret.value.trim() ? { state: "set", value: secret.value.trim() } : { state: "keep" },
            receive_private_and_group: privateGroup.checked,
            receive_guild: guild.checked,
            runtime_config: draft.runtime_config || {},
          },
          expected_revision: state.loginRevision ?? 0,
          dry_run: false,
        },
      });
      secret.value = "";
      await refresh();
    } catch (error) {
      state.reportError?.(error);
    } finally {
      apply.disabled = false;
    }
  };
  section.append(
    settingsRow("启用 QQ Bot", "启用后才会连接 QQ Gateway", enabled),
    settingsRow("App ID", "QQ 开放平台机器人 AppID；启用前必须填写", appId),
    settingsRow("Client Secret", `当前${productLabel(draft.client_secret?.state)}，保存后不会再次显示`, secret),
    settingsRow("接收私聊和群消息", "对应开放平台群/C2C 事件 intent", privateGroup),
    settingsRow("接收频道消息", "对应开放平台频道 @ 消息 intent", guild),
  );
  const actions = element("div", "actions");
  actions.append(apply);
  section.append(actions);
  return section;
}

function relatedCard() {
  const section = element("section", "card card--outlined");
  section.append(
    element("h2", "", "相关管理"),
    element("p", "muted", "会话策略、命令匹配和 Agent 会话由各自页面编辑。"),
  );
  const actions = element("div", "actions");
  actions.append(
    linkButton("会话策略", "config"),
    linkButton("命令与流程", "bot-flow"),
    linkButton("Agent", "agent-connections"),
  );
  section.append(actions);
  return section;
}

/** Mount the QQ operations panel into the shared console shell. */
export function mountQqBotPanel(host, rpc, events, options = {}) {
  const state = {
    rpc,
    snapshot: null,
    deliveries: [],
    interactions: [],
    deliveryCursor: null,
    interactionCursor: null,
    deliveryExpanded: false,
    interactionExpanded: false,
    actorId: options.actorId || "web-console",
    configAvailable: false,
    loginDraft: null,
    loginRevision: 0,
    ownerUnavailable: false,
  };
  host.innerHTML = "";
  const root = element("div", "qq-bot-panel settings-page stack");
  const search = element("input", "");
  search.type = "search";
  search.placeholder = "搜索账号、会话或投递";
  const status = element("div", "muted", "正在加载…");
  const refreshButton = element("button", "ghost", "刷新");
  refreshButton.type = "button";
  const toolbar = element("div", "toolbar row-item");
  toolbar.append(search, refreshButton, status);
  root.append(toolbar);
  host.append(root);

  state.reportError = (error) => {
    status.className = "error-banner";
    status.textContent = errorMessage(error);
  };

  function render() {
    const drafts = new Map();
    root.querySelectorAll("[data-account-id]").forEach((card) => {
      const values = {};
      card.querySelectorAll("[data-draft-field]").forEach((field) => {
        values[field.dataset.draftField] = field.value;
      });
      drafts.set(card.dataset.accountId, values);
    });
    root.querySelectorAll("section, article").forEach((node) => node.remove());
    root.append(loginCard(state, refresh));
    (state.snapshot?.accounts || []).forEach((account) => {
      const card = accountCard(account, state, rpc, refresh);
      const values = drafts.get(account.account_id);
      if (values) {
        card.querySelectorAll("[data-draft-field]").forEach((field) => {
          if (values[field.dataset.draftField] != null) field.value = values[field.dataset.draftField];
        });
        card.querySelector("[data-draft-field='scene']")?.onchange?.();
      }
      root.append(card);
    });
    root.append(relatedCard());
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
      state.deliveryExpanded = true;
      render();
    } catch (error) {
      if (!isOwnerUnavailable(error)) state.reportError(error);
    }
  }

  async function loadMoreInteractions(after) {
    try {
      const page = await rpc.read("qq-bot", "interactions.list", { query: search.value, after, limit: 50 });
      state.interactions.push(...page.items);
      state.interactionCursor = page.next_cursor;
      state.interactionExpanded = true;
      render();
    } catch (error) {
      if (!isOwnerUnavailable(error)) state.reportError(error);
    }
  }

  async function loadLogin() {
    const snapshot = await optionalCall(rpc, "config", "snapshot.read", {
      provider_id: QQ_PROVIDER_ID,
      context: { scope: "mutsuki.global" },
    });
    state.configAvailable = snapshot.ok;
    if (!snapshot.ok) {
      state.loginDraft = null;
      return;
    }
    state.loginDraft = snapshotDraft(snapshot.value);
    state.loginRevision = snapshot.value?.revision ?? 0;
  }

  const mergeRows = (fresh, existing, key) => {
    const seen = new Set(fresh.map(key));
    return [...fresh, ...existing.filter((item) => !seen.has(key(item)))];
  };
  let disposed = false;
  let pollTimer = null;
  let eventTimer = null;
  let searchTimer = null;
  let inFlight = null;
  let pendingRefresh = null;
  let lastRevision = 0;
  let opened = false;

  function schedule() {
    clearTimeout(pollTimer);
    if (!disposed && !document.hidden) pollTimer = setTimeout(() => void refresh(true), 60_000);
  }

  function refresh(merge = true) {
    if (disposed) return Promise.resolve();
    if (inFlight) {
      pendingRefresh = pendingRefresh === false ? false : merge;
      return inFlight;
    }
    const query = search.value;
    inFlight = (async () => {
      try {
        await loadLogin();
        try {
          const [snapshot, deliveries, interactions] = await Promise.all([
            rpc.read("qq-bot", "snapshot", { query }),
            rpc.read("qq-bot", "deliveries.list", { query, limit: 50 }),
            rpc.read("qq-bot", "interactions.list", { query, limit: 50 }),
          ]);
          if (disposed || query !== search.value) return;
          state.snapshot = snapshot;
          state.deliveries = merge && state.deliveryExpanded
            ? mergeRows(deliveries.items, state.deliveries, (item) => item.receipt.delivery_id)
            : deliveries.items;
          state.interactions = merge && state.interactionExpanded
            ? mergeRows(interactions.items, state.interactions, (item) => item.session_id)
            : interactions.items;
          if (!state.deliveryExpanded || !merge) state.deliveryCursor = deliveries.next_cursor;
          if (!state.interactionExpanded || !merge) state.interactionCursor = interactions.next_cursor;
          state.ownerUnavailable = false;
        } catch (error) {
          if (!isOwnerUnavailable(error)) throw error;
          if (disposed || query !== search.value) return;
          state.snapshot = { revision: 0, accounts: [] };
          state.deliveries = [];
          state.interactions = [];
          state.deliveryCursor = null;
          state.interactionCursor = null;
          state.ownerUnavailable = true;
        }
        if (disposed || query !== search.value) return;
        status.className = "muted";
        if (root.contains(document.activeElement) && document.activeElement?.matches?.("[data-draft-field]")) {
          status.textContent = "正在编辑，数据将在离开输入框后更新";
        } else {
          status.textContent = state.ownerUnavailable ? "QQ Bot 尚未启用，可先完成登录配置" : "";
          render();
        }
      } catch (error) {
        if (!disposed) state.reportError(error);
      }
    })().finally(() => {
      inFlight = null;
      if (pendingRefresh !== null) {
        const mergePending = pendingRefresh;
        pendingRefresh = null;
        void refresh(mergePending);
      } else {
        schedule();
      }
    });
    return inFlight;
  }

  search.oninput = () => {
    state.deliveryExpanded = false;
    state.interactionExpanded = false;
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      if (inFlight) pendingRefresh = false;
      else void refresh(false);
    }, 180);
  };
  refreshButton.onclick = () => void refresh(true);
  root.addEventListener("focusout", (event) => {
    if (!event.target?.matches?.("[data-draft-field]")) return;
    setTimeout(() => {
      if (!disposed && !(root.contains(document.activeElement) && document.activeElement?.matches?.("[data-draft-field]"))) {
        status.textContent = state.ownerUnavailable ? "QQ Bot 尚未启用，可先完成登录配置" : "";
        render();
      }
    }, 0);
  });
  const visibility = () => {
    clearTimeout(pollTimer);
    if (!document.hidden) void refresh(true);
  };
  const eventSubscription = events.subscribe("qq.changed", (payload) => {
    const revision = Number(payload?.revision || 0);
    if (revision <= lastRevision) return;
    lastRevision = revision;
    if (document.hidden) return;
    clearTimeout(eventTimer);
    eventTimer = setTimeout(() => void refresh(true), 50);
  }, "bot.read");
  const connectionSubscription = events.onStateChange?.((connection) => {
    if (connection !== "open" || document.hidden) return;
    if (opened) void refresh(true);
    opened = true;
  });
  document.addEventListener("visibilitychange", visibility);
  void refresh(false);
  return {
    refresh,
    destroy() {
      disposed = true;
      clearTimeout(searchTimer);
      clearTimeout(pollTimer);
      clearTimeout(eventTimer);
      eventSubscription.dispose();
      connectionSubscription?.dispose();
      document.removeEventListener("visibilitychange", visibility);
    },
  };
}

function registerConfigEditor(entry) {
  (globalThis.__mutsukiConfigEditors ??= new Map()).set(entry.providerId, entry);
}

export default {
  id: "qq-bot",
  setup(ctx) {
    ctx.pages.register({
      id: "qq-bot.page", path: "/qq-bot", title: "QQ 管理",
      component: { mount(el) { const panel = mountQqBotPanel(el, ctx.rpc, ctx.events); return { dispose: () => panel.destroy() }; } },
      requiredCapability: "bot.read",
    });
    registerConfigEditor({
      providerId: "mutsuki.bot.adapter.qqbot",
      activityId: "bot",
      pageId: "qq-bot.page",
      label: "打开 QQ 管理",
      mode: "supplement",
    });
    ctx.navigation.register({
      id: "qq-bot.nav", activityId: "bot", pageId: "qq-bot.page", label: "QQ 管理", order: 10,
      requiredCapability: "bot.read",
    });
  },
};
