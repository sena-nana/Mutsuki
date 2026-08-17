const STYLE = `
.sandbox-layout { display: grid; grid-template-columns: 240px minmax(0,1fr) 220px; gap: 12px; min-height: 560px; }
.sandbox-col { display: flex; flex-direction: column; min-height: 0; min-width: 0; }
.sandbox-list { overflow: auto; flex: 1; display: flex; flex-direction: column; gap: 6px; }
.sandbox-item { border: 1px solid var(--border, transparent); border-radius: 10px; padding: 10px 12px; background: var(--bg-elev, transparent); text-align: left; cursor: pointer; }
.sandbox-item.is-active { outline: 2px solid var(--accent, #7aa2ff); }
.sandbox-messages { flex: 1; overflow: auto; display: flex; flex-direction: column; gap: 8px; padding: 8px 0; }
.sandbox-bubble { max-width: 80%; border-radius: 12px; padding: 8px 12px; background: var(--bg-subtle, transparent); align-self: flex-start; }
.sandbox-bubble--bot { align-self: flex-end; background: var(--bg-hover, transparent); }
.sandbox-bubble--system { align-self: center; background: transparent; }
.sandbox-compose { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; }
.sandbox-compose input[type="text"] { flex: 1; min-width: 160px; }
@media (max-width: 960px) { .sandbox-layout { grid-template-columns: 1fr; min-height: 0; } }
`;

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
      if (typeof parsed.message === "string") return parsed.message;
    } catch (_) {}
  }
  return raw.startsWith("extension ") || raw.includes("rpc ") ? fallback : raw || fallback;
}

function kindLabel(kind) {
  return { group: "群聊", private: "私聊", channel: "频道" }[kind] || kind || "—";
}

/** Mount the QQ sandbox conversation console. */
export function mountSandboxPanel(host, rpc, events) {
  const state = { snapshot: null, messages: [], selectedId: null, draft: "", speakerId: "", inject: false };
  host.innerHTML = "";
  const style = document.createElement("style");
  style.textContent = STYLE;
  const status = element("div", "muted", "正在加载…");
  const modeSimulate = element("button", "ghost", "模拟");
  const modeLive = element("button", "ghost", "真实数据");
  const refreshButton = element("button", "ghost", "刷新");
  modeSimulate.type = modeLive.type = refreshButton.type = "button";
  const toolbar = element("div", "toolbar row-item");
  toolbar.append(modeSimulate, modeLive, refreshButton, status);
  const body = element("div", "sandbox-layout");
  const root = element("div", "sandbox-panel settings-page stack");
  root.append(toolbar, body);
  host.append(style, root);

  const reportError = (error) => {
    status.className = "error-banner";
    status.textContent = errorMessage(error);
  };
  const current = () => (state.snapshot?.conversations || []).find((item) => item.conversation_id === state.selectedId) || null;
  const write = (action, confirm = false) => rpc.write("sandbox", "write", {
    confirmed: confirm,
    request: { operation_id: crypto.randomUUID(), expected_revision: state.snapshot?.revision ?? 0, action },
  });

  function render() {
    const mode = state.snapshot?.mode || "simulate";
    modeSimulate.classList.toggle("is-active", mode === "simulate");
    modeLive.classList.toggle("is-active", mode === "live");
    body.replaceChildren();
    const conversations = element("section", "card card--outlined sandbox-col");
    conversations.append(element("h2", "", "会话"));
    const list = element("div", "sandbox-list");
    const items = state.snapshot?.conversations || [];
    if (!items.length) {
      list.append(element("p", "muted", mode === "live"
        ? (state.snapshot?.live_available ? "还没有观察到真实群聊发言" : "尚未连接 QQ，真实模式暂无数据")
        : "暂无模拟会话"));
    }
    items.forEach((conversation) => {
      const item = element("button", "sandbox-item");
      item.type = "button";
      if (conversation.conversation_id === state.selectedId) item.classList.add("is-active");
      item.append(
        element("strong", "", conversation.title || kindLabel(conversation.kind)),
        element("p", "muted", `${kindLabel(conversation.kind)} · ${conversation.users.length} 人 · ${conversation.last_preview || "—"}`),
      );
      item.onclick = () => { state.selectedId = conversation.conversation_id; void loadConversation(); };
      list.append(item);
    });
    conversations.append(list);

    const conversation = current();
    const chat = element("section", "card card--outlined sandbox-col");
    chat.append(element("h2", "", conversation ? conversation.title : "对话"));
    if (!conversation) {
      chat.append(element("p", "muted", "选择一个会话开始"));
    } else {
      const messages = element("div", "sandbox-messages");
      if (!state.messages.length) messages.append(element("p", "muted", "暂无消息"));
      state.messages.forEach((message) => {
        const bubble = element("div", `sandbox-bubble sandbox-bubble--${message.role}`);
        bubble.append(element("p", "muted", message.sender_name), element("p", "", message.text));
        messages.append(bubble);
      });
      const compose = element("div", "sandbox-compose");
      if (mode === "simulate") {
        const speaker = element("select", "ui-input");
        (conversation.users || []).forEach((user) => {
          const option = element("option", "", user.display_name || user.user_id);
          option.value = user.user_id;
          speaker.append(option);
        });
        const bot = element("option", "", "机器人");
        bot.value = "__bot__";
        speaker.append(bot);
        speaker.value = state.speakerId || speaker.options[0]?.value || "__bot__";
        speaker.onchange = () => { state.speakerId = speaker.value; };
        compose.append(speaker);
      }
      const input = element("input", "ui-input");
      input.type = "text";
      input.placeholder = mode === "live" ? "以后台机器人身份发送到真实会话" : "输入消息";
      input.value = state.draft;
      input.oninput = () => { state.draft = input.value; };
      const send = element("button", "ghost", mode === "live" ? "发送到 QQ" : "发送");
      send.type = "button";
      send.onclick = async () => {
        const text = state.draft.trim();
        if (!text) { status.className = "muted"; status.textContent = "请填写消息"; return; }
        const asBot = mode === "live" || state.speakerId === "__bot__";
        if (mode === "live" && !window.confirm("将以机器人身份向真实 QQ 会话发送消息，是否继续？")) return;
        try {
          await write(asBot
            ? { action: "send_as_bot", conversation_id: conversation.conversation_id, text }
            : { action: "ingest_as_user", conversation_id: conversation.conversation_id, user_id: state.speakerId, text, inject_into_flow: state.inject },
          mode === "live" || state.inject);
          state.draft = "";
          await refresh();
        } catch (error) {
          reportError(error);
        }
      };
      compose.append(input, send);
      if (mode === "simulate") {
        const inject = element("label", "muted");
        const box = document.createElement("input");
        box.type = "checkbox";
        box.checked = state.inject;
        box.onchange = () => { state.inject = box.checked; };
        inject.append(box, document.createTextNode(" 同时投递到 Bot 流程"));
        compose.append(inject);
      }
      chat.append(messages, compose);
    }

    const users = element("section", "card card--outlined sandbox-col");
    users.append(element("h2", "", "会话用户"));
    if (mode === "live") {
      users.append(element("p", "muted", "只显示近期发言或进出的用户，官方协议不能拉取完整群成员。"));
    }
    const userList = element("div", "sandbox-list");
    const roster = conversation?.users || [];
    if (!conversation) userList.append(element("p", "muted", "选择会话后显示用户"));
    else if (!roster.length) userList.append(element("p", "muted", "暂无用户"));
    else roster.forEach((user) => userList.append(element("p", "", `${user.display_name} · ${user.user_id}`)));
    users.append(userList);
    body.append(conversations, chat, users);
  }

  async function loadConversation() {
    if (!state.selectedId) { state.messages = []; render(); return; }
    try {
      state.messages = await rpc.read("sandbox", "messages", { conversation_id: state.selectedId }) || [];
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
        status.className = "muted";
        status.textContent = snapshot.mode === "live"
          ? (snapshot.live_available ? "真实数据模式" : "真实数据模式：尚未连接 QQ")
          : "模拟模式";
        await loadConversation();
      } catch (error) {
        if (!disposed) reportError(error);
      }
    })().finally(() => { inFlight = null; schedule(); });
    return inFlight;
  }

  modeSimulate.onclick = async () => { try { await write({ action: "set_mode", mode: "simulate" }); await refresh(); } catch (error) { reportError(error); } };
  modeLive.onclick = async () => { try { await write({ action: "set_mode", mode: "live" }); await refresh(); } catch (error) { reportError(error); } };
  refreshButton.onclick = () => void refresh();
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
