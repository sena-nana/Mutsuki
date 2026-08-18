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
.sandbox-avatar { width: 32px; height: 32px; border-radius: 50%; display: grid; place-items: center; font-size: 13px; font-weight: 650; color: var(--accent-text, #fff); background: var(--accent, #7aa2ff); flex: none; overflow: hidden; }
.sandbox-avatar--sm { width: 24px; height: 24px; font-size: 11px; }
img.sandbox-avatar { display: block; object-fit: cover; object-position: center; padding: 0; }
.sandbox-session-title, .sandbox-member-name { display: block; font-size: 13px; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sandbox-session-preview { margin: 1px 0 0; font-size: 11px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sandbox-session-time { font-size: 10px; white-space: nowrap; min-width: max-content; justify-self: end; align-self: start; padding-top: 1px; }
.sandbox-messages { padding: 16px 18px; display: flex; flex-direction: column; gap: 12px; background: var(--bg-subtle, transparent); }
.sandbox-row { display: flex; gap: 8px; max-width: 78%; align-items: center; }
.sandbox-row--user { align-self: flex-start; }
.sandbox-row--bot { align-self: flex-end; flex-direction: row-reverse; }
.sandbox-row--system { align-self: center; max-width: 90%; }
.sandbox-bubble { border-radius: 12px; padding: 8px 12px; background: var(--bg-elev, transparent); min-width: 0; }
.sandbox-row--bot .sandbox-bubble { background: var(--accent-soft, var(--bg-hover, transparent)); }
.sandbox-row--system .sandbox-bubble { background: transparent; }
.sandbox-client .sandbox-reply { height: 22px; padding: 0 6px; font-size: 11px; flex: none; }
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
.sandbox-mention { color: var(--accent, #7aa2ff); font-weight: 650; }
.sandbox-media { display: block; max-width: min(240px, 100%); max-height: 180px; border-radius: 8px; margin-top: 6px; }
.sandbox-file { display: block; margin-top: 6px; font-size: 12px; }
.sandbox-card { margin-top: 6px; min-width: 180px; max-width: 260px; border: 1px solid var(--border, transparent); border-radius: 10px; overflow: hidden; background: var(--bg, transparent); }
.sandbox-card img { display: block; width: 100%; max-height: 120px; object-fit: cover; }
.sandbox-card-body { padding: 8px 10px; display: flex; flex-direction: column; gap: 4px; }
.sandbox-card-title { font-size: 13px; font-weight: 650; }
.sandbox-card-desc, .sandbox-markdown, .sandbox-keyboard { font-size: 12px; white-space: pre-wrap; }
.sandbox-keyboard { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 6px; }
.sandbox-keyboard span { border: 1px solid var(--border, transparent); border-radius: 8px; padding: 4px 8px; font-size: 11px; }
.sandbox-compose-tools, .sandbox-draft-chips { display: flex; flex-wrap: wrap; gap: 6px; align-items: center; }
.sandbox-draft-chip { display: inline-flex; align-items: center; gap: 4px; border-radius: 999px; padding: 2px 8px; font-size: 11px; background: var(--bg-hover, transparent); }
.sandbox-mention-picker { display: flex; flex-direction: column; max-height: 160px; overflow: auto; border: 1px solid var(--border, transparent); border-radius: 10px; }
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

function avatar(name, className = "sandbox-avatar", avatarUrl) {
  const value = String(name || "?").trim();
  const initial = value ? value.slice(0, 1).toUpperCase() : "?";
  if (!avatarUrl) return element("span", className, initial);
  const img = remoteImage(className, value || "头像", avatarUrl);
  img.onerror = () => img.replaceWith(element("span", className, initial));
  return img;
}

function remoteImage(className, alt, src) {
  const img = element("img", className);
  img.alt = alt || "";
  img.setAttribute("referrerpolicy", "no-referrer");
  if (src) img.src = src;
  return img;
}

function userById(users, userId) {
  return (users || []).find((user) => user.user_id === userId);
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

function mentionName(users, userId) {
  return (users || []).find((user) => user.user_id === userId)?.display_name || userId || "";
}

function arkFields(payload) {
  const kv = payload?.kv || payload?.ark?.kv || [];
  const map = Object.fromEntries((Array.isArray(kv) ? kv : []).map((item) => [item?.key, item?.value || ""]));
  return {
    title: map["#METATITLE#"] || map["#PROMPT#"] || payload?.title || "小卡片",
    desc: map["#METADESC#"] || payload?.description || "",
    image: map["#PIC#"] || map["#METAIMAGE#"] || payload?.image || "",
    url: map["#METAURL#"] || payload?.url || "",
  };
}

function bindMedia(node, mediaId, rpc) {
  void rpc.read("sandbox", "media.get", { media_id: mediaId }).then((blob) => {
    const binary = Uint8Array.from(atob(blob.bytes || ""), (ch) => ch.charCodeAt(0));
    node.src = URL.createObjectURL(new Blob([binary], { type: blob.mime || "application/octet-stream" }));
  }).catch(() => {
    node.replaceWith(document.createTextNode("[媒体不可用]"));
  });
}

function renderSegments(message, messages, users, rpc) {
  const wrap = document.createDocumentFragment();
  if (message.reply_to) wrap.append(element("p", "sandbox-quote muted", quotedText(messages, message.reply_to)));
  const body = element("div", "sandbox-rich");
  const segments = Array.isArray(message.segments) && message.segments.length
    ? message.segments
    : [{ type: "text", text: message.text || "" }];
  const hasAttachment = segments.some((segment) => segment.type === "platform_specific" && segment.kind === "attachment");
  segments.forEach((segment) => {
    if (segment.type === "text") body.append(document.createTextNode(segment.text || ""));
    else if (segment.type === "mention_user") body.append(element("span", "sandbox-mention", `@${mentionName(users, segment.user_id)}`));
    else if (segment.type === "mention_all") body.append(element("span", "sandbox-mention", "@全体成员"));
    else if (hasAttachment && (segment.type === "image" || segment.type === "file" || segment.type === "audio" || segment.type === "video")) return;
    else if (segment.type === "image") {
      const mediaId = segment.resource?.ref_id;
      if (!mediaId) body.append(document.createTextNode("[图片]"));
      else {
        const img = remoteImage("sandbox-media", "图片");
        bindMedia(img, mediaId, rpc);
        body.append(img);
      }
    } else if (segment.type === "file") body.append(element("span", "sandbox-file", `[${segment.name || "文件"}]`));
    else if (segment.type === "audio" || segment.type === "video") {
      const node = document.createElement(segment.type);
      node.className = "sandbox-media";
      node.controls = true;
      node.setAttribute("referrerpolicy", "no-referrer");
      const mediaId = segment.resource?.ref_id;
      if (mediaId) bindMedia(node, mediaId, rpc);
      body.append(node);
    } else if (segment.type === "platform_specific") renderPlatform(body, segment, rpc);
    else if (segment.type !== "reply" && segment.type !== "quote") body.append(document.createTextNode(message.text || ""));
  });
  if (!body.textContent && !body.querySelector("img, audio, video, .sandbox-card") && !message.reply_to) {
    body.textContent = message.text || "";
  }
  wrap.append(body);
  return wrap;
}

function renderPlatform(body, segment, rpc) {
  const kind = segment.kind || "";
  const payload = segment.payload || {};
  if (kind === "media" || kind === "attachment") {
    const mime = payload.mime || payload.content_type || "";
    const url = payload.url;
    const mediaId = payload.media_id;
    if (mime.startsWith("audio/") || mime.startsWith("video/")) {
      const node = document.createElement(mime.startsWith("audio/") ? "audio" : "video");
      node.className = "sandbox-media";
      node.controls = true;
      node.setAttribute("referrerpolicy", "no-referrer");
      if (url) node.src = url;
      else if (mediaId) bindMedia(node, mediaId, rpc);
      body.append(node);
      return;
    }
    if (mime.startsWith("image/") || kind === "attachment" || kind === "media") {
      const img = remoteImage("sandbox-media", payload.name || payload.filename || "图片", url);
      if (!url && mediaId) bindMedia(img, mediaId, rpc);
      body.append(img);
      return;
    }
    body.append(element("span", "sandbox-file", `[${payload.name || payload.filename || "文件"}]`));
    return;
  }
  if (kind === "ark" || kind === "embed") {
    const fields = arkFields(payload);
    const card = element("div", "sandbox-card");
    if (fields.image) {
      card.append(remoteImage("", fields.title, fields.image));
    }
    const inner = element("div", "sandbox-card-body");
    inner.append(element("span", "sandbox-card-title", fields.title));
    if (fields.desc) inner.append(element("span", "muted sandbox-card-desc", fields.desc));
    if (fields.url) inner.append(element("span", "muted sandbox-card-desc", fields.url));
    card.append(inner);
    body.append(card);
    return;
  }
  if (kind === "markdown") {
    const markdown = element("pre", "sandbox-markdown");
    markdown.textContent = payload.content || payload.markdown?.content || JSON.stringify(payload);
    body.append(markdown);
    return;
  }
  if (kind === "keyboard") {
    const bar = element("div", "sandbox-keyboard");
    const rows = payload.content?.rows || payload.rows || [];
    rows.flatMap((row) => row.buttons || []).forEach((item) => {
      bar.append(element("span", "", item.render_data?.label || item.label || "按钮"));
    });
    if (!bar.childNodes.length) bar.append(element("span", "", "[按钮]"));
    body.append(bar);
    return;
  }
  body.append(document.createTextNode(`[${kind}]`));
}

/** Mount the QQ sandbox conversation console. */
export function mountSandboxPanel(host, rpc, events) {
  const state = { snapshot: null, messages: [], selectedId: null, draft: "", draftSegments: [], speakerId: "", query: "", quote: null, dialog: "" };
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
      const peer = (conversation.users || [])[0];
      item.append(avatar(conversation.title || kindLabel(conversation.kind), "sandbox-avatar", conversation.avatar_url || peer?.avatar_url), meta, element("span", "muted sandbox-session-time", formatTime(conversation.last_activity_unix_ms)));
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
      if (message.role !== "system") {
        const sender = userById(conversation.users, message.sender_id);
        row.append(avatar(message.sender_name, "sandbox-avatar sandbox-avatar--sm", sender?.avatar_url));
      }
      const bubble = element("div", "sandbox-bubble");
      if (message.role !== "system") bubble.append(element("p", "muted", `${message.sender_name} · ${formatTime(message.time_ms)}`));
      bubble.append(renderSegments(message, state.messages, conversation.users, rpc));
      row.append(bubble);
      if (message.role === "user") {
        const reply = button("回复");
        reply.classList.add("ghost", "sandbox-reply");
        reply.title = "引用并回复这条消息";
        reply.onclick = (event) => {
          event.stopPropagation();
          state.quote = message;
          render();
        };
        row.append(reply);
      }
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
    if (state.draftSegments.length) {
      const chips = element("div", "sandbox-draft-chips");
      state.draftSegments.forEach((segment, index) => {
        const chip = element("span", "sandbox-draft-chip", draftLabel(segment, conversation.users));
        const remove = button("×");
        remove.onclick = () => { state.draftSegments.splice(index, 1); render(); };
        chip.append(remove);
        chips.append(chip);
      });
      compose.append(chips);
    }
    const tools = element("div", "sandbox-compose-tools");
    const mentionBtn = button("@");
    mentionBtn.title = "艾特成员";
    mentionBtn.onclick = () => { state.draft += "@"; render(); };
    tools.append(mentionBtn);
    if (mode() === "simulate") {
      const imageBtn = button("图片");
      imageBtn.onclick = () => void attachFile("image/*");
      const fileBtn = button("文件");
      fileBtn.onclick = () => void attachFile("*/*");
      const cardBtn = button("小卡片");
      cardBtn.onclick = () => openDialog("card");
      const markdownBtn = button("Markdown");
      markdownBtn.onclick = () => openDialog("markdown");
      tools.append(imageBtn, fileBtn, cardBtn, markdownBtn);
    }
    compose.append(tools);
    const picker = element("div", "sandbox-mention-picker");
    picker.hidden = true;
    const row = element("div", "sandbox-compose-row");
    const input = element("input", "ui-input");
    input.type = "text";
    const canActive = Boolean(conversation.active_message);
    input.placeholder = mode() === "live"
      ? (canActive ? "可直接发送主动消息，或点右侧回复" : "请先点用户消息右侧回复")
      : "输入消息，Enter 发送";
    input.value = state.draft;
    const refreshPicker = () => {
      const match = /@([^\s@]*)$/.exec(state.draft);
      picker.replaceChildren();
      if (!match) { picker.hidden = true; return; }
      const query = match[1].toLowerCase();
      const hits = [{ user_id: "__all__", display_name: "全体成员" }, ...(conversation.users || [])]
        .filter((user) => (user.display_name || "").toLowerCase().includes(query) || user.user_id.toLowerCase().includes(query));
      hits.forEach((user) => {
        const item = button(`@${user.display_name || user.user_id}`);
        item.onclick = () => {
          state.draft = state.draft.slice(0, match.index);
          state.draftSegments.push(user.user_id === "__all__"
            ? { type: "mention_all" }
            : { type: "mention_user", user_id: user.user_id });
          render();
        };
        picker.append(item);
      });
      picker.hidden = !hits.length;
    };
    input.oninput = () => { state.draft = input.value; refreshPicker(); };
    const send = button(mode() === "live" ? "发送到 QQ" : "发送");
    const submit = async () => {
      const text = state.draft.trim();
      const segments = state.draftSegments.slice();
      if (!text && !segments.length) { showStatus("请填写消息"); return; }
      if (mode() === "live" && !state.quote?.message_id && !canActive) {
        showStatus("当前会话没有主动消息权限，请先点击用户消息右侧的回复");
        return;
      }
      if (mode() === "live" && !window.confirm("将以机器人身份向真实 QQ 会话发送消息，是否继续？")) return;
      try {
        if (mode() === "live") {
          await write({
            action: "send_as_bot",
            conversation_id: conversation.conversation_id,
            text,
            segments,
            reply_to: state.quote?.message_id || null,
          }, true);
        } else {
          const speaker = state.speakerId || conversation.users?.[0]?.user_id;
          if (!speaker) { showStatus("请先添加用户"); return; }
          await write({
            action: "ingest_as_user",
            conversation_id: conversation.conversation_id,
            user_id: speaker,
            text,
            segments,
            reply_to: state.quote?.message_id || null,
          });
        }
        state.draft = "";
        state.draftSegments = [];
        state.quote = null;
        showStatus("");
        await refresh();
      } catch (error) {
        reportError(error);
      }
    };
    send.onclick = () => void submit();
    input.onkeydown = (event) => { if (event.key === "Enter") { event.preventDefault(); void submit(); } };
    row.append(input, send);
    compose.append(picker, row);
    refreshPicker();
    pane.append(compose);
  }

  async function attachFile(accept) {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = accept;
    input.onchange = async () => {
      const file = input.files?.[0];
      if (!file) return;
      try {
        const buffer = new Uint8Array(await file.arrayBuffer());
        let binary = "";
        buffer.forEach((byte) => { binary += String.fromCharCode(byte); });
        const uploaded = await rpc.write("sandbox", "media.upload", {
          name: file.name,
          mime: file.type || "application/octet-stream",
          bytes: btoa(binary),
        });
        state.draftSegments.push({
          type: "platform_specific",
          platform: "sandbox",
          kind: "media",
          payload: { media_id: uploaded.media_id, mime: uploaded.mime, name: uploaded.name },
        });
        showStatus("");
        render();
      } catch (error) {
        reportError(error);
      }
    };
    input.click();
  }

  function draftLabel(segment, users) {
    if (segment.type === "mention_user") return `@${mentionName(users, segment.user_id)}`;
    if (segment.type === "mention_all") return "@全体成员";
    if (segment.type === "platform_specific" && segment.kind === "ark") return "[小卡片]";
    if (segment.type === "platform_specific" && segment.kind === "markdown") return "[Markdown]";
    if (segment.type === "platform_specific" && segment.kind === "media") return `[${segment.payload?.name || "媒体"}]`;
    return segment.type || "附件";
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
    if (dialog === "card") {
      card.setAttribute("aria-label", "发送小卡片");
      head.append(element("h2", "", "发送小卡片"));
      const title = element("input", "ui-input");
      title.placeholder = "标题";
      const desc = element("input", "ui-input");
      desc.placeholder = "描述";
      const image = element("input", "ui-input");
      image.placeholder = "封面 URL（可选）";
      const link = element("input", "ui-input");
      link.placeholder = "跳转链接（可选）";
      const titleField = element("label", "sandbox-dialog-field", "标题");
      const descField = element("label", "sandbox-dialog-field", "描述");
      const imageField = element("label", "sandbox-dialog-field", "封面");
      const linkField = element("label", "sandbox-dialog-field", "链接");
      titleField.append(title);
      descField.append(desc);
      imageField.append(image);
      linkField.append(link);
      body.append(titleField, descField, imageField, linkField);
      const save = button("加入草稿");
      save.onclick = () => {
        if (!title.value.trim()) { showStatus("请填写卡片标题"); return; }
        state.draftSegments.push({
          type: "platform_specific",
          platform: "qqbot",
          kind: "ark",
          payload: {
            template_id: 23,
            kv: [
              { key: "#METATITLE#", value: title.value.trim() },
              { key: "#METADESC#", value: desc.value.trim() },
              { key: "#PIC#", value: image.value.trim() },
              { key: "#METAURL#", value: link.value.trim() },
            ],
          },
        });
        closeDialog();
        render();
      };
      foot.append(cancel, save);
      card.append(head, body, foot);
      overlay.append(card);
      root.append(overlay);
      title.focus();
      return;
    }
    if (dialog === "markdown") {
      card.setAttribute("aria-label", "发送 Markdown");
      head.append(element("h2", "", "发送 Markdown"));
      const content = document.createElement("textarea");
      content.className = "ui-input";
      content.rows = 6;
      content.placeholder = "Markdown 文本";
      const field = element("label", "sandbox-dialog-field", "内容");
      field.append(content);
      body.append(field);
      const save = button("加入草稿");
      save.onclick = () => {
        if (!content.value.trim()) { showStatus("请填写 Markdown"); return; }
        state.draftSegments.push({
          type: "platform_specific",
          platform: "qqbot",
          kind: "markdown",
          payload: { content: content.value },
        });
        closeDialog();
        render();
      };
      foot.append(cancel, save);
      card.append(head, body, foot);
      overlay.append(card);
      root.append(overlay);
      content.focus();
      return;
    }
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
      item.append(avatar(user.display_name || user.user_id, "sandbox-avatar sandbox-avatar--sm", user.avatar_url), meta);
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
      item.append(avatar(user.display_name || user.user_id, "sandbox-avatar sandbox-avatar--sm", user.avatar_url), meta);
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
