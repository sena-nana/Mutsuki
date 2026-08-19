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

export function mountQqAccountCards(host, rpc, events) {
  ensureStyle();
  const root = element("div", "stack");
  host.replaceChildren(root);
  let disposed = false;
  let pollTimer = null;
  let inFlight = null;

  function render(snapshot, ownerUnavailable) {
    root.replaceChildren();
    if (ownerUnavailable) {
      root.append(element("p", "muted", "尚未登录 QQ，请到配置里填写账号。"));
      return;
    }
    (snapshot?.accounts || []).forEach((account) => root.append(accountCard(account)));
  }

  function schedule() {
    clearTimeout(pollTimer);
    if (!disposed && !document.hidden) pollTimer = setTimeout(() => void refresh(), 60_000);
  }

  function refresh() {
    if (disposed || inFlight) return Promise.resolve();
    inFlight = rpc
      .read("qq-bot", "snapshot", {})
      .then((snapshot) => {
        if (!disposed) render(snapshot, false);
      })
      .catch((error) => {
        if (disposed) return;
        if (isOwnerUnavailable(error)) render({ accounts: [] }, true);
        else root.replaceChildren(element("p", "error-banner", errorMessage(error)));
      })
      .finally(() => {
        inFlight = null;
        schedule();
      });
    return inFlight;
  }

  const visibility = () => {
    clearTimeout(pollTimer);
    if (!document.hidden) void refresh();
  };
  const changed = events.subscribe("qq.changed", () => {
    if (!document.hidden) void refresh();
  }, "bot.read");
  const connection = events.onStateChange?.((state) => {
    if (state === "open" && !document.hidden) void refresh();
  });
  document.addEventListener("visibilitychange", visibility);
  void refresh();
  return {
    destroy() {
      disposed = true;
      clearTimeout(pollTimer);
      changed.dispose();
      connection?.dispose();
      document.removeEventListener("visibilitychange", visibility);
    },
  };
}

function registerConfigEditor(entry) {
  (globalThis.__mutsukiConfigEditors ??= new Map()).set(entry.providerId, entry);
}

export default {
  id: "qq-bot",
  setup() {
    registerConfigEditor({
      providerId: QQ_PROVIDER_ID,
      activityId: "home",
      pageId: "overview.page",
      label: "查看连接状态",
      mode: "supplement",
    });
  },
};
