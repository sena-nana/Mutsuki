const STYLE = `
.sandbox-client { display: flex; flex-direction: column; min-height: 620px; height: calc(100vh - 140px); min-width: 0; }
.sandbox-frame { flex: 1; min-height: 0; display: grid; grid-template-columns: 280px minmax(0,1fr) 220px; border: 1px solid var(--border, transparent); border-radius: 16px; overflow: hidden; }
.sandbox-pane { display: flex; flex-direction: column; min-width: 0; min-height: 0; }
.sandbox-pane + .sandbox-pane { border-left: 1px solid var(--border, transparent); }
.sandbox-pane-head { display: flex; align-items: center; gap: 8px; padding: 12px 14px; border-bottom: 1px solid var(--border, transparent); }
.sandbox-pane-head h2 { margin: 0; font-size: 15px; font-weight: 650; flex: 1; }
.sandbox-session-list, .sandbox-member-list, .sandbox-messages { flex: 1; overflow: auto; }
.sandbox-session, .sandbox-member { display: grid; gap: 8px; align-items: center; width: 100%; border: 0; background: transparent; text-align: left; padding: 8px 14px; cursor: pointer; color: inherit; }
.sandbox-session { grid-template-columns: 40px minmax(0,1fr) auto; padding: 10px 14px; }
.sandbox-member { grid-template-columns: 28px minmax(0,1fr) auto; }
.sandbox-session:hover, .sandbox-member:hover { background: var(--bg-hover, var(--bg-subtle, transparent)); }
.sandbox-session.is-active, .sandbox-member.is-active { background: var(--accent-soft, var(--bg-hover, transparent)); }
.sandbox-avatar { width: 40px; height: 40px; border-radius: 50%; display: grid; place-items: center; font-weight: 650; color: var(--accent-text, #fff); background: var(--accent, #7aa2ff); flex: none; }
.sandbox-avatar--sm { width: 28px; height: 28px; font-size: 12px; }
.sandbox-session-title, .sandbox-member-name { display: block; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sandbox-session-preview { margin: 2px 0 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sandbox-messages { padding: 16px 18px; display: flex; flex-direction: column; gap: 12px; background: var(--bg-subtle, transparent); }
.sandbox-row { display: flex; gap: 8px; max-width: 78%; align-items: flex-start; }
.sandbox-row--user { align-self: flex-start; }
.sandbox-row--bot { align-self: flex-end; flex-direction: row-reverse; }
.sandbox-row--system { align-self: center; max-width: 90%; }
.sandbox-bubble { border-radius: 12px; padding: 8px 12px; background: var(--bg-elev, transparent); }
.sandbox-row--bot .sandbox-bubble { background: var(--accent-soft, var(--bg-hover, transparent)); }
.sandbox-row--system .sandbox-bubble { background: transparent; }
.sandbox-quote { margin: 0 0 6px; padding: 4px 8px; border-left: 3px solid var(--accent, #7aa2ff); opacity: 0.8; font-size: 12px; }
.sandbox-compose { display: flex; flex-direction: column; gap: 8px; padding: 12px 14px; border-top: 1px solid var(--border, transparent); }
.sandbox-compose-row { display: flex; gap: 8px; align-items: center; }
.sandbox-compose-row input[type="text"] { flex: 1; min-width: 0; }
.sandbox-quote-bar { display: flex; align-items: center; justify-content: space-between; gap: 8px; font-size: 13px; }
.sandbox-empty { padding: 24px 16px; }
@media (max-width: 960px) {
  .sandbox-client { height: auto; }
  .sandbox-frame { grid-template-columns: 1fr; min-height: 720px; }
  .sandbox-pane + .sandbox-pane { border-left: 0; border-top: 1px solid var(--border, transparent); }
  .sandbox-session-list { max-height: 220px; }
  .sandbox-member-list { max-height: 180px; }
  .sandbox-messages { min-height: 280px; }
}
`;

function element(tag, className, content) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (content !== undefined) node.textContent = content;
  return node;
}

function button(label) {
  const node = element("button", "ghost", label);
  node.type = "button";
  return node;
}

function errorMessage(error, fallback = "操作失败，请稍后重试") {
  const raw = error?.message || String(error ?? "");
  const start = raw.indexOf("{");
  if (start >= 0) {
    try {
      const parsed = JSON.parse(raw.slice(start));
      if (typeof parsed.message === "string") return parsed.message;
    } catch (_) {}
  }
  return raw.startsWith("extension ") || raw.includes("rpc ") ? fallback : raw || fallback;
}

function kindLabel(kind) {
  return { group: "群聊", private: "私聊", channel: "频道" }[kind] || kind || "会话";
}

function avatar(name, className = "sandbox-avatar") {
  const value = String(name || "?").trim();
  return element("span", className, value ? value.slice(0, 1).toUpperCase() : "?");
}

function formatTime(unixMs) {
  const date = new Date(Number(unixMs));
  if (!unixMs || Number.isNaN(date.getTime())) return "";
  return date.toDateString() === new Date().toDateString()
    ? date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
    : date.toLocaleDateString();
}

function quotedText(messages, replyTo) {
  const quoted = (messages || []).find((item) => item.message_id === replyTo);
  return quoted ? `${quoted.sender_name}: ${quoted.text || "消息"}` : "引用消息";
}

function renderSegments(message, messages) {
  const wrap = document.createDocumentFragment();
  if (message.reply_to) wrap.append(element("p", "sandbox-quote muted", quotedText(messages, message.reply_to)));
  const body = element("p", "");
  const segments = Array.isArray(message.segments) && message.segments.length
    ? message.segments
    : [{ type: "text", text: message.text || "" }];
  segments.forEach((segment) => {
    if (segment.type === "text") body.append(document.createTextNode(segment.text || ""));
    else if (segment.type === "mention_user") body.append(element("strong", "", `@${segment.user_id || ""}`));
    else if (segment.type === "mention_all") body.append(element("strong", "", "@全体成员"));
    else if (segment.type === "image") body.append(document.createTextNode("[图片]"));
    else if (segment.type === "file") body.append(document.createTextNode(`[${segment.name || "文件"}]`));
    else if (segment.type === "audio") body.append(document.createTextNode("[语音]"));
    else if (segment.type === "video") body.append(document.createTextNode("[视频]"));
  });
  if (!body.textContent && !message.reply_to) body.textContent = message.text || "";
  wrap.append(body);
  return wrap;
}

/** Mount the QQ sandbox conversation console. */
export function mountSandboxPanel(host, rpc, events) {
  const state = { snapshot: null, messages: [], selectedId: null, draft: "", speakerId: "", query: "", quote: null };
  host.innerHTML = "";
  const style = document.createElement("style");
  style.textContent = STYLE;
  const status = element("div", "muted", "正在加载…");
  const modeSimulate = button("模拟");
  const modeLive = button("真实数据");
  const addUser = button("添加用户");
  const clearChat = button("清空");
  const toolbar = element("div", "toolbar row-item");
  toolbar.append(modeSimulate, modeLive, addUser, clearChat, status);
  const frame = element("div", "sandbox-frame");
  const root = element("div", "sandbox-panel sandbox-client settings-page stack");
  root.append(toolbar, frame);
  host.append(style, root);

  const reportError = (error) => { status.className = "error-banner"; status.textContent = errorMessage(error); };
  const current = () => (state.snapshot?.conversations || []).find((item) => item.conversation_id === state.selectedId) || null;
  const mode = () => state.snapshot?.mode || "simulate";
  const write = (action, confirm = false) => rpc.write("sandbox", "write", {
    confirmed: confirm,
    request: { operation_id: crypto.randomUUID(), expected_revision: state.snapshot?.revision ?? 0, action },
  });
  const setStatus = () => {
    status.className = "muted";
    status.textContent = mode() === "live"
      ? (state.snapshot?.live_available ? "真实数据模式" : "真实数据模式：尚未连接 QQ")
      : (state.snapshot?.flow_available ? "模拟模式：发言进入 Bot 流程" : "模拟模式：Bot 流程当前不可用");
  };

  function renderSessions(pane) {
    const head = element("div", "sandbox-pane-head");
    head.append(element("h2", "", "会话"));
    pane.append(head);
    const search = element("input", "ui-input");
    search.type = "search";
    search.placeholder = "搜索会话";
    search.value = state.query;
    search.oninput = () => { state.query = search.value; render(); };
    pane.append(search);
    const list = element("div", "sandbox-session-list");
    const query = state.query.trim().toLowerCase();
    const items = (state.snapshot?.conversations || []).filter((conversation) => !query
      || (conversation.title || "").toLowerCase().includes(query)
      || (conversation.users || []).some((user) => (user.display_name || "").toLowerCase().includes(query)));
    if (!items.length) {
      list.append(element("p", "muted sandbox-empty", mode() === "live"
        ? (state.snapshot?.live_available ? "还没有观察到真实群聊发言" : "尚未连接 QQ，真实模式暂无数据")
        : "点击「添加用户」开始与机器人对话"));
    }
    items.forEach((conversation) => {
      const item = element("button", "sandbox-session");
      item.type = "button";
      if (conversation.conversation_id === state.selectedId) item.classList.add("is-active");
      const meta = element("div", "");
      meta.append(
        element("span", "sandbox-session-title", conversation.title || kindLabel(conversation.kind)),
        element("p", "muted sandbox-session-preview", `${kindLabel(conversation.kind)} · ${conversation.last_preview || "暂无消息"}`),
      );
      item.append(avatar(conversation.title || kindLabel(conversation.kind)), meta, element("span", "muted", formatTime(conversation.last_activity_unix_ms)));
      item.onclick = () => { state.selectedId = conversation.conversation_id; state.quote = null; void loadConversation(); };
      list.append(item);
    });
    pane.append(list);
  }

  function renderMessages(pane, conversation) {
    const head = element("div", "sandbox-pane-head");
    head.append(element("h2", "", conversation ? conversation.title : "对话"));
    if (conversation) head.append(element("span", "muted", kindLabel(conversation.kind)));
    pane.append(head);
    const messages = element("div", "sandbox-messages");
    if (!conversation) messages.append(element("p", "muted sandbox-empty", "选择一个会话开始"));
    else if (!state.messages.length) messages.append(element("p", "muted sandbox-empty", "暂无消息"));
    else state.messages.forEach((message) => {
      const row = element("div", `sandbox-row sandbox-row--${message.role}`);
      if (message.role !== "system") row.append(avatar(message.sender_name, "sandbox-avatar sandbox-avatar--sm"));
      const bubble = element("div", "sandbox-bubble");
      if (message.role !== "system") bubble.append(element("p", "muted", `${message.sender_name} · ${formatTime(message.time_ms)}`));
      bubble.append(renderSegments(message, state.messages));
      if (mode() === "simulate" && message.role !== "system") {
        bubble.style.cursor = "pointer";
        bubble.title = "点击引用这条消息";
        bubble.onclick = () => { state.quote = message; render(); };
      }
      row.append(bubble);
      messages.append(row);
    });
    pane.append(messages);
    if (!conversation) return;
    const compose = element("div", "sandbox-compose");
    if (state.quote) {
      const bar = element("div", "sandbox-quote-bar muted");
      const cancel = button("取消");
      cancel.onclick = () => { state.quote = null; render(); };
      bar.append(element("span", "", `正在回复 ${state.quote.sender_name}`), cancel);
      compose.append(bar);
    }
    const row = element("div", "sandbox-compose-row");
    const input = element("input", "ui-input");
    input.type = "text";
    input.placeholder = mode() === "live" ? "以后台机器人身份发送到真实会话" : "输入消息，Enter 发送";
    input.value = state.draft;
    input.oninput = () => { state.draft = input.value; };
    const send = button(mode() === "live" ? "发送到 QQ" : "发送");
    const submit = async () => {
      const text = state.draft.trim();
      if (!text) { status.className = "muted"; status.textContent = "请填写消息"; return; }
      if (mode() === "live" && !window.confirm("将以机器人身份向真实 QQ 会话发送消息，是否继续？")) return;
      try {
        if (mode() === "live") {
          await write({ action: "send_as_bot", conversation_id: conversation.conversation_id, text }, true);
        } else {
          const speaker = state.speakerId || conversation.users?.[0]?.user_id;
          if (!speaker) { status.className = "muted"; status.textContent = "请先添加用户"; return; }
          await write({
            action: "ingest_as_user",
            conversation_id: conversation.conversation_id,
            user_id: speaker,
            text,
            reply_to: state.quote?.message_id || null,
          });
        }
        state.draft = "";
        state.quote = null;
        await refresh();
      } catch (error) {
        reportError(error);
      }
    };
    send.onclick = () => void submit();
    input.onkeydown = (event) => { if (event.key === "Enter") { event.preventDefault(); void submit(); } };
    row.append(input, send);
    compose.append(row);
    pane.append(compose);
  }

  function renderMembers(pane, conversation) {
    const head = element("div", "sandbox-pane-head");
    head.append(element("h2", "", "成员"));
    pane.append(head);
    pane.append(element("p", "muted sandbox-empty", mode() === "live"
      ? "只显示近期发言或进出的用户，官方协议不能拉取完整群成员。"
      : "点击成员切换发言身份"));
    const list = element("div", "sandbox-member-list");
    const roster = conversation?.users || [];
    if (!conversation) list.append(element("p", "muted sandbox-empty", "选择会话后显示成员"));
    else if (!roster.length) list.append(element("p", "muted sandbox-empty", "暂无成员"));
    else roster.forEach((user) => {
      const item = element("button", "sandbox-member");
      item.type = "button";
      if (mode() === "simulate" && user.user_id === state.speakerId) item.classList.add("is-active");
      const meta = element("div", "");
      meta.append(element("span", "sandbox-member-name", user.display_name || user.user_id), element("p", "muted sandbox-session-preview", user.user_id));
      item.append(avatar(user.display_name || user.user_id, "sandbox-avatar sandbox-avatar--sm"), meta);
      if (mode() === "simulate") {
        const remove = button("移除");
        remove.onclick = async (event) => {
          event.stopPropagation();
          try {
            await write({ action: "remove_user", user_id: user.user_id });
            if (state.speakerId === user.user_id) state.speakerId = "";
            await refresh();
          } catch (error) {
            reportError(error);
          }
        };
        item.append(remove);
        item.onclick = () => { state.speakerId = user.user_id; render(); };
      }
      list.append(item);
    });
    pane.append(list);
  }

  function render() {
    modeSimulate.classList.toggle("is-active", mode() === "simulate");
    modeLive.classList.toggle("is-active", mode() === "live");
    addUser.hidden = mode() !== "simulate";
    clearChat.hidden = mode() !== "simulate" || !state.selectedId;
    const sessions = element("section", "sandbox-pane");
    const chat = element("section", "sandbox-pane");
    const members = element("section", "sandbox-pane");
    renderSessions(sessions);
    renderMessages(chat, current());
    renderMembers(members, current());
    frame.replaceChildren(sessions, chat, members);
  }

  async function loadConversation() {
    if (!state.selectedId) { state.messages = []; render(); return; }
    try {
      state.messages = await rpc.read("sandbox", "messages", { conversation_id: state.selectedId }) || [];
      const conversation = current();
      if (mode() === "simulate" && conversation?.users?.length && !conversation.users.some((user) => user.user_id === state.speakerId)) {
        state.speakerId = conversation.users[0].user_id;
      }
      render();
    } catch (error) {
      reportError(error);
    }
  }

  let disposed = false;
  let pollTimer = null;
  let inFlight = null;
  const schedule = () => {
    clearTimeout(pollTimer);
    if (!disposed && !document.hidden) pollTimer = setTimeout(() => void refresh(), 15_000);
  };
  function refresh() {
    if (disposed) return Promise.resolve();
    if (inFlight) return inFlight;
    inFlight = (async () => {
      try {
        const snapshot = await rpc.read("sandbox", "snapshot", { query: "" });
        if (disposed) return;
        state.snapshot = snapshot;
        if (!state.selectedId || !(snapshot.conversations || []).some((item) => item.conversation_id === state.selectedId)) {
          state.selectedId = snapshot.conversations?.[0]?.conversation_id || null;
        }
        setStatus();
        await loadConversation();
      } catch (error) {
        if (!disposed) reportError(error);
      }
    })().finally(() => { inFlight = null; schedule(); });
    return inFlight;
  }

  const act = (action) => async () => { try { await write(action); await refresh(); } catch (error) { reportError(error); } };
  modeSimulate.onclick = act({ action: "set_mode", mode: "simulate" });
  modeLive.onclick = act({ action: "set_mode", mode: "live" });
  addUser.onclick = act({ action: "add_user" });
  clearChat.onclick = async () => {
    if (!state.selectedId) return;
    try { await write({ action: "clear_conversation", conversation_id: state.selectedId }); await refresh(); } catch (error) { reportError(error); }
  };
  const visibility = () => { clearTimeout(pollTimer); if (!document.hidden) void refresh(); };
  const eventSubscription = events.subscribe("sandbox.changed", () => { if (!document.hidden) void refresh(); }, "bot.read");
  document.addEventListener("visibilitychange", visibility);
  void refresh();
  return {
    refresh,
    destroy() {
      disposed = true;
      clearTimeout(pollTimer);
      eventSubscription.dispose();
      document.removeEventListener("visibilitychange", visibility);
    },
  };
}

export default {
  id: "sandbox",
  setup(ctx) {
    ctx.pages.register({
      id: "sandbox.page", path: "/sandbox", title: "QQ 沙盒",
      component: { mount(el) { const panel = mountSandboxPanel(el, ctx.rpc, ctx.events); return { dispose: () => panel.destroy() }; } },
      requiredCapability: "bot.read",
    });
    ctx.navigation.register({
      id: "sandbox.nav", activityId: "bot", pageId: "sandbox.page", label: "QQ 沙盒", order: 15,
      requiredCapability: "bot.read",
    });
  },
};
