const STYLE = `
.sandbox-client { position: relative; display: flex; flex-direction: column; min-height: 620px; height: calc(100vh - 140px); min-width: 0; }
.sandbox-frame { flex: 1; min-height: 0; display: grid; grid-template-columns: minmax(0, 220px) minmax(0, 1fr) minmax(0, 168px); border: 1px solid var(--border, transparent); border-radius: 16px; overflow: hidden; }
.sandbox-pane { display: flex; flex-direction: column; min-width: 0; min-height: 0; overflow: hidden; }
.sandbox-pane + .sandbox-pane { border-left: 1px solid var(--border, transparent); }
.sandbox-pane-head { display: flex; align-items: center; gap: 6px; padding: 8px 10px; border-bottom: 1px solid var(--border, transparent); flex: 0 0 auto; }
.sandbox-pane-head h2 { margin: 0; font-size: 13px; font-weight: 650; flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sandbox-mode-tabs { width: 100%; height: auto; }
.sandbox-mode-tabs button { flex: 1; justify-content: center; height: 26px; font-size: 12px; }
.sandbox-status { margin: 0; padding: 0 10px 6px; font-size: 12px; }
.sandbox-status:empty, .sandbox-status[hidden] { display: none; }
.sandbox-search-row { display: flex; align-items: center; gap: 6px; padding: 8px 8px 6px; min-width: 0; flex: 0 0 auto; }
.sandbox-search-row input[type="search"] { flex: 1; min-width: 0; width: auto; height: 28px; margin: 0; box-sizing: border-box; font-size: 12px; }
.sandbox-client .sandbox-add {
  box-sizing: border-box; width: 28px; min-width: 28px; max-width: 28px; height: 28px; padding: 0;
  flex: none; font-size: 16px; line-height: 1; border-radius: 8px;
}
.sandbox-clear { margin-left: auto; flex: none; }
.sandbox-session-list, .sandbox-member-list, .sandbox-messages { flex: 1; min-height: 0; overflow: auto; }
.sandbox-client .sandbox-session, .sandbox-client .sandbox-member {
  display: grid; gap: 8px; align-items: center; justify-content: stretch; justify-items: start;
  width: 100%; height: auto; min-height: 40px; box-sizing: border-box; overflow: hidden;
  border: 0; border-radius: 0; background: transparent; text-align: left; padding: 6px 10px; cursor: pointer; color: inherit;
}
.sandbox-session { grid-template-columns: 32px minmax(0, 1fr) max-content; }
.sandbox-member { grid-template-columns: 24px minmax(0, 1fr); }
.sandbox-session > *, .sandbox-member > * { min-width: 0; }
.sandbox-session > div, .sandbox-member > div { overflow: hidden; }
.sandbox-session p, .sandbox-member p, .sandbox-bubble p { margin: 0; }
.sandbox-session-title, .sandbox-session-preview, .sandbox-member-name { max-width: 100%; }
.sandbox-session:hover, .sandbox-member:hover { background: var(--bg-hover, var(--bg-subtle, transparent)); }
.sandbox-session.is-active, .sandbox-member.is-active { background: var(--accent-soft, var(--bg-hover, transparent)); }
.sandbox-avatar { width: 32px; height: 32px; border-radius: 50%; display: grid; place-items: center; font-size: 13px; font-weight: 650; color: var(--accent-text, #fff); background: var(--accent, #7aa2ff); flex: none; }
.sandbox-avatar--sm { width: 24px; height: 24px; font-size: 11px; }
.sandbox-session-title, .sandbox-member-name { display: block; font-size: 13px; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sandbox-session-preview { margin: 1px 0 0; font-size: 11px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sandbox-session-time { font-size: 10px; white-space: nowrap; min-width: max-content; justify-self: end; align-self: start; padding-top: 1px; }
.sandbox-messages { padding: 16px 18px; display: flex; flex-direction: column; gap: 12px; background: var(--bg-subtle, transparent); }
.sandbox-row { display: flex; gap: 8px; max-width: 78%; align-items: flex-start; }
.sandbox-row--user { align-self: flex-start; }
.sandbox-row--bot { align-self: flex-end; flex-direction: row-reverse; }
.sandbox-row--system { align-self: center; max-width: 90%; }
.sandbox-bubble { border-radius: 12px; padding: 8px 12px; background: var(--bg-elev, transparent); min-width: 0; }
.sandbox-row--bot .sandbox-bubble { background: var(--accent-soft, var(--bg-hover, transparent)); }
.sandbox-row--system .sandbox-bubble { background: transparent; }
.sandbox-quote { margin: 0 0 6px; padding: 4px 8px; border-left: 3px solid var(--accent, #7aa2ff); opacity: 0.8; font-size: 12px; }
.sandbox-compose { display: flex; flex-direction: column; gap: 8px; padding: 12px 14px; border-top: 1px solid var(--border, transparent); flex: 0 0 auto; }
.sandbox-compose-row { display: flex; gap: 8px; align-items: center; min-width: 0; }
.sandbox-compose-row input[type="text"] { flex: 1; min-width: 0; }
.sandbox-quote-bar { display: flex; align-items: center; justify-content: space-between; gap: 8px; font-size: 13px; }
.sandbox-client .sandbox-member .ghost { height: 22px; padding: 0 6px; font-size: 11px; flex: none; }
.sandbox-member-actions { display: flex; gap: 4px; flex-wrap: wrap; }
.sandbox-dialog-overlay { position: absolute; inset: 0; z-index: var(--z-dialog, 1800); display: grid; place-items: center; padding: 16px; box-sizing: border-box; background: color-mix(in oklch, black 42%, transparent); }
.sandbox-dialog { width: min(380px, 100%); max-height: min(520px, 100%); min-height: 0; display: flex; flex-direction: column; background: var(--lilia-surface-fill-raised, var(--bg-elev, var(--bg, #fff))); border: 1px solid var(--border, transparent); border-radius: 16px; box-shadow: var(--shadow-dialog, 0 14px 40px rgb(0 0 0 / 0.35)); overflow: hidden; }
.sandbox-dialog-head, .sandbox-dialog-foot { display: flex; align-items: center; gap: 8px; padding: 12px 14px; flex: none; }
.sandbox-dialog-head { border-bottom: 1px solid var(--border, transparent); }
.sandbox-dialog-head h2 { margin: 0; font-size: 14px; font-weight: 650; flex: 1; min-width: 0; }
.sandbox-dialog-body { padding: 12px 14px; overflow: auto; min-height: 0; display: flex; flex-direction: column; gap: 10px; }
.sandbox-dialog-field { display: flex; flex-direction: column; gap: 4px; font-size: 12px; color: var(--text-muted); }
.sandbox-dialog-field input { width: 100%; min-width: 0; height: 32px; margin: 0; box-sizing: border-box; }
.sandbox-dialog-foot { justify-content: flex-end; border-top: 1px solid var(--border, transparent); }
.sandbox-dialog .sandbox-member { border-radius: 8px; padding: 8px 10px; }
.sandbox-dialog .sandbox-member:disabled { opacity: 0.45; cursor: default; }
.sandbox-empty { padding: 10px; margin: 0; font-size: 12px; }
@media (max-width: 960px) {
  .sandbox-client { height: auto; min-height: 0; }
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
  const state = { snapshot: null, messages: [], selectedId: null, draft: "", speakerId: "", query: "", quote: null, dialog: "" };
  host.innerHTML = "";
  const style = document.createElement("style");
  style.textContent = STYLE;
  const status = element("p", "muted sandbox-status");
  status.hidden = true;
  const frame = element("div", "sandbox-frame");
  const root = element("div", "sandbox-panel sandbox-client settings-page stack");
  root.append(frame);
  host.append(style, root);

  const showStatus = (text, kind = "muted") => {
    status.className = `${kind} sandbox-status`;
    status.textContent = text;
    status.hidden = !text;
  };
  const reportError = (error) => showStatus(errorMessage(error), "error-banner");
  const current = () => (state.snapshot?.conversations || []).find((item) => item.conversation_id === state.selectedId) || null;
  const mode = () => state.snapshot?.mode || "simulate";
  const write = (action, confirm = false) => rpc.write("sandbox", "write", {
    confirmed: confirm,
    request: { operation_id: crypto.randomUUID(), expected_revision: state.snapshot?.revision ?? 0, action },
  });
  const setStatus = () => showStatus("");

  function renderSessions(pane) {
    const tabs = element("div", "segmented sandbox-mode-tabs");
    const modeSimulate = element("button", "", "模拟");
    const modeLive = element("button", "", "真实数据");
    modeSimulate.type = modeLive.type = "button";
    modeSimulate.classList.toggle("is-active", mode() === "simulate");
    modeLive.classList.toggle("is-active", mode() === "live");
    modeSimulate.onclick = async () => { try { await write({ action: "set_mode", mode: "simulate" }); await refresh(); } catch (error) { reportError(error); } };
    modeLive.onclick = async () => { try { await write({ action: "set_mode", mode: "live" }); await refresh(); } catch (error) { reportError(error); } };
    tabs.append(modeSimulate, modeLive);
    const head = element("div", "sandbox-pane-head");
    head.append(tabs);
    pane.append(head, status);
    const searchRow = element("div", "sandbox-search-row");
    const search = element("input", "ui-input");
    search.type = "search";
    search.placeholder = "搜索会话";
    search.value = state.query;
    search.oninput = () => { state.query = search.value; render(); };
    searchRow.append(search);
    if (mode() === "simulate") {
      const addUser = button("+");
      addUser.className = "ghost sandbox-add";
      addUser.setAttribute("aria-label", "添加用户");
      addUser.title = "添加用户";
      addUser.onclick = async () => { try { await write({ action: "add_user" }); await refresh(); } catch (error) { reportError(error); } };
      searchRow.append(addUser);
    }
    pane.append(searchRow);
    const list = element("div", "sandbox-session-list");
    const query = state.query.trim().toLowerCase();
    const items = (state.snapshot?.conversations || []).filter((conversation) => !query
      || (conversation.title || "").toLowerCase().includes(query)
      || (conversation.users || []).some((user) => (user.display_name || "").toLowerCase().includes(query)));
    if (!items.length) list.append(element("p", "muted sandbox-empty", "暂无会话"));
    items.forEach((conversation) => {
      const item = element("button", "sandbox-session");
      item.type = "button";
      if (conversation.conversation_id === state.selectedId) item.classList.add("is-active");
      const meta = element("div", "");
      meta.append(
        element("span", "sandbox-session-title", conversation.title || kindLabel(conversation.kind)),
        element("p", "muted sandbox-session-preview", `${kindLabel(conversation.kind)} · ${conversation.last_preview || "暂无消息"}`),
      );
      item.append(avatar(conversation.title || kindLabel(conversation.kind)), meta, element("span", "muted sandbox-session-time", formatTime(conversation.last_activity_unix_ms)));
      item.onclick = () => { state.selectedId = conversation.conversation_id; state.quote = null; void loadConversation(); };
      list.append(item);
    });
    pane.append(list);
  }

  function renderMessages(pane, conversation) {
    const head = element("div", "sandbox-pane-head");
    head.append(element("h2", "", conversation ? conversation.title : "对话"));
    if (conversation) head.append(element("span", "muted", kindLabel(conversation.kind)));
    if (mode() === "simulate" && conversation) {
      const clearChat = button("清空");
      clearChat.classList.add("sandbox-clear");
      clearChat.onclick = async () => {
        try {
          await write({ action: "clear_conversation", conversation_id: conversation.conversation_id });
          await refresh();
        } catch (error) {
          reportError(error);
        }
      };
      head.append(clearChat);
    }
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
      if (!text) { showStatus("请填写消息"); return; }
      if (mode() === "live" && !window.confirm("将以机器人身份向真实 QQ 会话发送消息，是否继续？")) return;
      try {
        if (mode() === "live") {
          await write({ action: "send_as_bot", conversation_id: conversation.conversation_id, text }, true);
        } else {
          const speaker = state.speakerId || conversation.users?.[0]?.user_id;
          if (!speaker) { showStatus("请先添加用户"); return; }
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

  function closeDialog() {
    state.dialog = "";
    root.querySelector(".sandbox-dialog-overlay")?.remove();
  }

  function openDialog(dialog) {
    if (mode() !== "simulate") { closeDialog(); return; }
    state.dialog = dialog;
    const existing = root.querySelector(".sandbox-dialog-overlay");
    if (existing?.dataset.dialog === dialog) return;
    existing?.remove();
    const overlay = element("div", "sandbox-dialog-overlay");
    overlay.dataset.dialog = dialog;
    overlay.onclick = () => closeDialog();
    const card = element("div", "sandbox-dialog");
    card.setAttribute("role", "dialog");
    card.setAttribute("aria-modal", "true");
    card.onclick = (event) => event.stopPropagation();
    const head = element("div", "sandbox-dialog-head");
    const body = element("div", "sandbox-dialog-body");
    const foot = element("div", "sandbox-dialog-foot");
    const cancel = button("取消");
    cancel.onclick = () => closeDialog();
    if (dialog !== "import") {
      const user = (state.snapshot?.conversations || []).flatMap((item) => item.users || []).find((item) => item.user_id === dialog);
      if (!user) { closeDialog(); return; }
      card.setAttribute("aria-label", "编辑成员");
      head.append(element("h2", "", "编辑成员"));
      const openid = element("input", "ui-input");
      openid.placeholder = "OpenID";
      openid.value = user.user_id;
      const nickname = element("input", "ui-input");
      nickname.placeholder = "昵称";
      nickname.value = user.display_name || "";
      const openidField = element("label", "sandbox-dialog-field", "OpenID");
      const nicknameField = element("label", "sandbox-dialog-field", "昵称");
      openidField.append(openid);
      nicknameField.append(nickname);
      body.append(openidField, nicknameField);
      const save = button("保存");
      save.onclick = async () => {
        const newId = openid.value.trim();
        const wasPrivate = current()?.conversation?.user_id === user.user_id;
        try {
          await write({ action: "update_user", user_id: user.user_id, new_user_id: newId, display_name: nickname.value.trim() });
          if (state.speakerId === user.user_id) state.speakerId = newId;
          closeDialog();
          await refresh();
          if (wasPrivate) {
            const next = (state.snapshot?.conversations || []).find((item) => item.conversation?.user_id === newId);
            if (next) { state.selectedId = next.conversation_id; await loadConversation(); }
          }
        } catch (error) {
          reportError(error);
        }
      };
      foot.append(cancel, save);
      card.append(head, body, foot);
      overlay.append(card);
      root.append(overlay);
      openid.focus();
      openid.select();
      return;
    }
    const liveUsers = state.snapshot?.live_users || [];
    const taken = new Set((current()?.users || []).map((user) => user.user_id));
    card.setAttribute("aria-label", "导入成员");
    head.append(element("h2", "", "导入成员"));
    if (!liveUsers.length) body.append(element("p", "muted sandbox-empty", "真实数据还没有成员可导入"));
    liveUsers.forEach((user) => {
      const item = element("button", "sandbox-member");
      item.type = "button";
      item.disabled = taken.has(user.user_id);
      const meta = element("div", "");
      meta.append(element("span", "sandbox-member-name", user.display_name || user.user_id), element("p", "muted sandbox-session-preview", user.user_id));
      item.append(avatar(user.display_name || user.user_id, "sandbox-avatar sandbox-avatar--sm"), meta);
      item.onclick = async () => {
        try {
          await write({ action: "import_live_users", user_ids: [user.user_id] });
          closeDialog();
          await refresh();
        } catch (error) {
          reportError(error);
        }
      };
      body.append(item);
    });
    const importAll = button("全部导入");
    importAll.onclick = async () => {
      const user_ids = liveUsers.map((user) => user.user_id).filter((id) => !taken.has(id));
      if (!user_ids.length) { showStatus("没有可导入的成员"); return; }
      try {
        await write({ action: "import_live_users", user_ids });
        closeDialog();
        await refresh();
      } catch (error) {
        reportError(error);
      }
    };
    foot.append(cancel);
    if (liveUsers.length) foot.append(importAll);
    card.append(head, body, foot);
    overlay.append(card);
    root.append(overlay);
  }

  function renderMembers(pane, conversation) {
    if (mode() !== "simulate") closeDialog();
    const head = element("div", "sandbox-pane-head");
    head.append(element("h2", "", "成员"));
    if (mode() === "simulate") {
      const importMembers = button("导入");
      importMembers.classList.add("sandbox-clear");
      importMembers.onclick = () => openDialog("import");
      head.append(importMembers);
    }
    pane.append(head);
    const list = element("div", "sandbox-member-list");
    const roster = conversation?.users || [];
    if (conversation && !roster.length) list.append(element("p", "muted sandbox-empty", "暂无成员"));
    roster.forEach((user) => {
      const item = element("div", "sandbox-member");
      item.setAttribute("role", "button");
      item.tabIndex = 0;
      if (mode() === "simulate" && user.user_id === state.speakerId) item.classList.add("is-active");
      const meta = element("div", "");
      meta.append(element("span", "sandbox-member-name", user.display_name || user.user_id), element("p", "muted sandbox-session-preview", user.user_id));
      if (mode() === "simulate") {
        const actions = element("div", "sandbox-member-actions");
        const edit = button("编辑");
        edit.onclick = (event) => { event.stopPropagation(); openDialog(user.user_id); };
        const remove = button("移除");
        remove.onclick = async (event) => {
          event.stopPropagation();
          try {
            await write({ action: "remove_user", user_id: user.user_id });
            if (state.speakerId === user.user_id) state.speakerId = "";
            if (state.dialog === user.user_id) closeDialog();
            await refresh();
          } catch (error) {
            reportError(error);
          }
        };
        actions.append(edit, remove);
        meta.append(actions);
        item.onclick = () => { state.speakerId = user.user_id; render(); };
        item.onkeydown = (event) => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); state.speakerId = user.user_id; render(); } };
      }
      item.append(avatar(user.display_name || user.user_id, "sandbox-avatar sandbox-avatar--sm"), meta);
      list.append(item);
    });
    pane.append(list);
  }

  function render() {
    const sessions = element("section", "sandbox-pane");
    const chat = element("section", "sandbox-pane");
    const members = element("section", "sandbox-pane");
    renderSessions(sessions);
    renderMessages(chat, current());
    renderMembers(members, current());
    frame.replaceChildren(sessions, chat, members);
    if (state.dialog) openDialog(state.dialog);
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

  const onKey = (event) => {
    if (event.key === "Escape") closeDialog();
  };
  const visibility = () => { clearTimeout(pollTimer); if (!document.hidden) void refresh(); };
  const eventSubscription = events.subscribe("sandbox.changed", () => { if (!document.hidden) void refresh(); }, "bot.read");
  document.addEventListener("visibilitychange", visibility);
  document.addEventListener("keydown", onKey);
  void refresh();
  return {
    refresh,
    destroy() {
      disposed = true;
      clearTimeout(pollTimer);
      eventSubscription.dispose();
      document.removeEventListener("visibilitychange", visibility);
      document.removeEventListener("keydown", onKey);
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
