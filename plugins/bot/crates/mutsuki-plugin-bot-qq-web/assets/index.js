const QQ_PROVIDER_ID = "mutsuki.bot.adapter.qqbot";

const STYLE = `
.qq-account-head { display: flex; align-items: center; gap: 12px; margin-bottom: 8px; }
.qq-account-avatar { width: 48px; height: 48px; border-radius: 50%; display: grid; place-items: center; font-size: 18px; font-weight: 650; color: var(--accent-text, #fff); background: var(--accent, #7aa2ff); flex: none; overflow: hidden; object-fit: cover; object-position: center; }
img.qq-account-avatar { display: block; padding: 0; }
.qq-account-title { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.qq-account-title h3 { margin: 0; }
.qq-account-head p { margin: 2px 0 0; }
.qq-account-status { display: inline-flex; align-items: center; padding: 4px 10px; border-radius: 8px; border: 1px solid var(--border, #d0d7de); font-size: 12px; font-weight: 600; line-height: 1.2; }
.qq-account-status.is-ok { color: var(--ok); border-color: color-mix(in oklch, var(--ok) 32%, var(--border, #d0d7de)); background: var(--ok-soft); }
.qq-account-status.is-warn { color: var(--warn); border-color: color-mix(in oklch, var(--warn) 32%, var(--border, #d0d7de)); background: var(--warn-soft); }
.qq-account-status.is-err { color: var(--err); border-color: color-mix(in oklch, var(--err) 32%, var(--border, #d0d7de)); background: var(--err-soft); }
`;

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

function formatDuration(unixMs) {
  const started = Number(unixMs);
  if (!unixMs || Number.isNaN(started) || started <= 0) return "—";
  const minutes = Math.floor(Math.max(0, Date.now() - started) / 60_000);
  const hours = Math.floor(minutes / 60);
  const days = Math.floor(hours / 24);
  if (days > 0) return `${days} 天 ${hours % 24} 小时`;
  if (hours > 0) return `${hours} 小时 ${minutes % 60} 分`;
  if (minutes > 0) return `${minutes} 分钟`;
  return "不到 1 分钟";
}

function ensureStyle() {
  if (document.getElementById("qq-bot-self-style")) return;
  const style = element("style");
  style.id = "qq-bot-self-style";
  style.textContent = STYLE;
  document.head.append(style);
}

function httpsQqCdn(url) {
  const value = String(url || "").trim();
  const href = value.startsWith("//")
    ? `https:${value}`
    : value.slice(0, 7).toLowerCase() === "http://"
      ? `https://${value.slice(7)}`
      : "";
  try {
    return href && /(^|\.)(qlogo\.cn|qpic\.cn|gtimg\.cn|qq\.com\.cn|qq\.com)$/i.test(new URL(href).hostname)
      ? href
      : value;
  } catch (_) {
    return value;
  }
}

function avatar(name, className, avatarUrl) {
  const value = String(name || "?").trim();
  const initial = value ? value.slice(0, 1).toUpperCase() : "?";
  if (!avatarUrl) return element("span", className, initial);
  const img = element("img", className);
  img.alt = value || "头像";
  img.setAttribute("referrerpolicy", "no-referrer");
  img.src = httpsQqCdn(avatarUrl);
  img.onerror = () => img.replaceWith(element("span", className, initial));
  return img;
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

function accountCard(account) {
  const card = element("article", "card card--outlined");
  const selfUser = account.self_user || {};
  const title = selfUser.display_name || account.account_id || "QQ 账号";
  const status = element(
    "span",
    `qq-account-status ${account.health === "ok" ? "is-ok" : account.health === "degraded" ? "is-warn" : "is-err"}`,
    `${productLabel(account.health)} · ${productLabel(account.connection_state)}`,
  );
  const heading = element("div", "qq-account-title");
  heading.append(element("h3", "", title), status);
  const meta = element("div");
  meta.append(heading);
  if (selfUser.user_id) meta.append(element("p", "muted", `OpenID ${selfUser.user_id}`));
  meta.append(element("p", "muted", `在线时长 ${formatDuration(account.connected_since_unix_ms)}`));
  const head = element("div", "qq-account-head");
  head.append(avatar(title, "qq-account-avatar", selfUser.avatar_url), meta);
  card.append(head);
  if (account.last_error) card.append(element("p", "error-banner", account.last_error));
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

/** Mount the QQ operations panel into the shared console shell. */
export function mountQqBotPanel(host, rpc, events, options = {}) {
  ensureStyle();
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
    root.querySelectorAll("section, article").forEach((node) => node.remove());
    if (state.ownerUnavailable) {
      root.append(element("p", "muted", "尚未登录 QQ，请到配置里填写账号。"));
      return;
    }
    (state.snapshot?.accounts || []).forEach((account) => root.append(accountCard(account)));
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
        status.textContent = state.ownerUnavailable ? "尚未登录 QQ，请到配置里填写账号" : "";
        render();
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
      id: "qq-bot.page", path: "/qq-bot", title: "QQ 连接",
      component: { mount(el) { const panel = mountQqBotPanel(el, ctx.rpc, ctx.events); return { dispose: () => panel.destroy() }; } },
      requiredCapability: "bot.read",
    });
    registerConfigEditor({
      providerId: QQ_PROVIDER_ID,
      activityId: "bot",
      pageId: "qq-bot.page",
      label: "查看连接状态",
      mode: "supplement",
    });
    ctx.navigation.register({
      id: "qq-bot.nav", activityId: "bot", pageId: "qq-bot.page", label: "QQ 连接", order: 10,
      requiredCapability: "bot.read",
    });
  },
};
