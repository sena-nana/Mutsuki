import {
  EVENT_PAGE_LIMIT,
  EVENT_WINDOW_LIMIT,
  asEventPage,
  mergeBySequence,
  projectAgentEvents,
  sequenceOf,
  trimWindow,
} from "./trajectory-model.js";
import { mountTrajectoryView } from "./trajectory-view.js";

function esc(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function errorText(error) {
  const raw = error && typeof error === "object" && error.message
    ? error.message
    : String(error ?? "");
  const start = raw.indexOf("{");
  if (start >= 0) {
    try {
      const parsed = JSON.parse(raw.slice(start));
      if (typeof parsed.message === "string" && parsed.message.trim()) return parsed.message;
    } catch (_) {}
  }
  return raw.startsWith("extension ") || raw.includes("rpc ")
    ? "操作失败，请稍后重试"
    : raw || "操作失败，请稍后重试";
}

export async function mountAgentConnectionsPanel(el, rpc) {
  el.innerHTML = `<div class="card"><div class="toolbar nested"><h2>Agent 状态</h2><button id="agent-provider-test" class="ghost" hidden>测试模型</button><span id="agent-provider-test-result" class="muted"></span></div><div id="agent-connection-list"></div></div>
    <div class="card"><div class="toolbar nested"><h3>会话</h3><button id="agent-session-refresh" class="ghost" hidden>刷新</button></div><div id="agent-session-list"></div><div id="agent-session-detail"></div></div>`;
  let revision = 0;
  let sessions = [];
  let nextSessionId = null;
  const list = el.querySelector("#agent-connection-list");
  const sessionList = el.querySelector("#agent-session-list");
  const detail = el.querySelector("#agent-session-detail");

  async function refresh() {
    const body = await rpc.call("bot-agent", "connections.snapshot", {});
    const snapshot = body.snapshot || body;
    revision = snapshot.revision || 0;
    const items = snapshot.connections || [];
    list.innerHTML = items.length ? items.map((item) => `<div class="tree-item row-item"><div><strong>${item.connection_id === "local" ? "本机 Agent" : esc(item.connection_id)}</strong></div><div class="row-actions"><span class="pill ${item.state === "healthy" ? "ok" : "warn"}">${item.state === "healthy" ? "运行中" : "不可用"}</span>${item.connection_id === "local" ? "" : `<button class="ghost" data-reconnect="${esc(item.connection_id)}">重连</button>`}</div></div>`).join("") : `<div class="muted">Agent 尚未启用</div>`;
    list.querySelectorAll("[data-reconnect]").forEach((button) => {
      button.onclick = async () => {
        try {
          await rpc.call("bot-agent", "connections.reconnect", { expected_revision: revision, connection_id: button.dataset.reconnect });
          await refresh();
        } catch (error) { list.innerHTML = `<div class="muted">${esc(errorText(error))}</div>`; }
      };
    });
  }

  async function fetchSessionPage(sessionId, afterSequence) {
    const body = await rpc.call("bot-agent", "sessions.events", {
      session_id: sessionId,
      after_sequence: afterSequence,
      limit: EVENT_PAGE_LIMIT,
    });
    return asEventPage(body);
  }

  async function showSession(sessionId) {
    try {
      const [sessionBody, stateBody] = await Promise.all([
        rpc.call("bot-agent", "sessions.get", { session_id: sessionId }),
        rpc.call("bot-agent", "sessions.state", { session_id: sessionId }),
      ]);
      const session = sessionBody.session || sessionBody;
      const state = stateBody.session_state || stateBody;
      const nextSeq = Number(session.next_event_sequence || 0);
      const tailAfter = Math.max(0, nextSeq - EVENT_PAGE_LIMIT);
      let pageMeta = { lost: 0, truncated: false };
      let events = [];
      const firstPage = await fetchSessionPage(sessionId, tailAfter);
      events = firstPage.events;
      pageMeta = firstPage;
      const messages = (session.messages || []).map((message) => `<div class="tree-item"><strong>${message.role === "user" ? "用户" : message.role === "assistant" ? "助手" : esc(message.role)}</strong><div>${esc(message.content)}</div></div>`).join("");
      const approvals = (state.pending_approvals || []).map((item) => `<div class="tree-item row-item"><div>${esc(item.request.summary || item.request.tool)}</div><div class="row-actions"><button data-approve="${esc(item.request.action_id)}">批准</button><button class="ghost" data-reject="${esc(item.request.action_id)}">拒绝</button></div></div>`).join("");
      const interactions = (state.pending_interactions || []).map((item) => `<div class="tree-item"><div>${esc(item.prompt)}</div><div class="toolbar nested"><input data-answer="${esc(item.interaction_id)}" placeholder="输入回答"><button data-interact="${esc(item.interaction_id)}">回答</button></div></div>`).join("");
      const activeTurn = (state.turns || []).find((turn) => ["collecting_context", "generating", "running_tools"].includes(turn.status));
      detail.innerHTML = `<div class="subsection"><h3>${esc(session.title || session.session_id)}</h3><div class="muted">${esc(session.turn_count)} 轮 · ${esc(state.usage?.total_tokens || 0)} tokens</div><h4>消息</h4>${messages || `<div class="muted">暂无消息</div>`}${approvals}${interactions}<h4>时间线</h4><div id="agent-trajectory" class="trajectory"></div><div class="toolbar nested">${activeTurn ? `<button id="agent-cancel-turn">停止当前回复</button>` : `<button class="ghost" id="agent-close-session">关闭会话</button>`}</div></div>`;
      const trajHost = detail.querySelector("#agent-trajectory");
      const traj = mountTrajectoryView(trajHost, trajectoryOptions());

      function firstSequence() {
        const values = events.map(sequenceOf).filter((value) => value != null);
        return values.length ? Math.min(...values) : 1;
      }

      function trajectoryOptions(loadingOlder = false) {
        const projected = projectAgentEvents(events);
        return {
          records: projected.records,
          groups: projected.groups,
          hasOlder: firstSequence() > 1 || !!pageMeta.truncated,
          loadingOlder,
          lost: pageMeta.lost,
          truncated: pageMeta.truncated,
          emptyText: "暂无时间线记录",
          onInspect: async (record) => {
            const sequences = (record.sequences || []).slice(0, 8);
            const details = [];
            for (const sequence of sequences) {
              const body = await rpc.call("bot-agent", "sessions.event_get", {
                session_id: sessionId,
                sequence,
              });
              details.push(body.event ? body : body);
            }
            return details;
          },
          onLoadOlder: async () => {
            const first = firstSequence();
            if (first <= 1 && !pageMeta.truncated) return;
            traj.update(trajectoryOptions(true));
            try {
              const olderAfter = Math.max(0, first - EVENT_PAGE_LIMIT - 1);
              const olderPage = await fetchSessionPage(sessionId, olderAfter);
              const older = olderPage.events.filter((event) => {
                const seq = sequenceOf(event);
                return seq != null && seq < first;
              });
              events = trimWindow(mergeBySequence(events, older), EVENT_WINDOW_LIMIT, "older");
              pageMeta = {
                lost: Number(olderPage.lost || 0),
                truncated: !!olderPage.truncated,
              };
              traj.update(trajectoryOptions(false));
            } catch (error) {
              traj.update({ ...trajectoryOptions(false), emptyText: errorText(error) });
            }
          },
        };
      }
      detail.querySelectorAll("[data-approve]").forEach((button) => button.onclick = async () => {
        const item = state.pending_approvals.find((entry) => entry.request.action_id === button.dataset.approve);
        await rpc.call("bot-agent", "sessions.approve", { decision: { session_id: item.request.session_id, turn_id: item.request.turn_id, action_id: item.request.action_id, version: item.request.version, decision: "approved" } });
        await showSession(sessionId);
      });
      detail.querySelectorAll("[data-reject]").forEach((button) => button.onclick = async () => {
        const item = state.pending_approvals.find((entry) => entry.request.action_id === button.dataset.reject);
        await rpc.call("bot-agent", "sessions.reject", { decision: { session_id: item.request.session_id, turn_id: item.request.turn_id, action_id: item.request.action_id, version: item.request.version, decision: "rejected" } });
        await showSession(sessionId);
      });
      detail.querySelectorAll("[data-interact]").forEach((button) => button.onclick = async () => {
        const item = state.pending_interactions.find((entry) => entry.interaction_id === button.dataset.interact);
        const answer = detail.querySelector(`[data-answer="${CSS.escape(item.interaction_id)}"]`).value;
        await rpc.call("bot-agent", "sessions.interact", { resolution: { session_id: item.session_id, turn_id: item.turn_id, version: item.version, interaction_id: item.interaction_id, accepted: true, response: { answer } } });
        await showSession(sessionId);
      });
      const cancelButton = detail.querySelector("#agent-cancel-turn");
      if (cancelButton) cancelButton.onclick = async () => {
        await rpc.call("bot-agent", "sessions.cancel_turn", { session_id: sessionId, turn_id: activeTurn.turn_id, expected_version: state.version?.[0] ?? state.version });
        await showSession(sessionId);
      };
      const closeButton = detail.querySelector("#agent-close-session");
      if (closeButton) closeButton.onclick = async () => {
          await rpc.call("bot-agent", "sessions.close", { session_id: sessionId, expected_version: state.version?.[0] ?? state.version });
          detail.innerHTML = "";
          await refreshSessions();
        };
    } catch (error) { detail.innerHTML = `<div class="muted">${esc(errorText(error))}</div>`; }
  }

  function renderSessions() {
    sessionList.innerHTML = sessions.length ? sessions.map((item) => `<button class="tree-item row-item ghost" data-session="${esc(item.session_id)}"><span>${esc(item.title || item.session_id)}</span><span class="muted">${esc(item.turn_count)} 轮 · ${esc(item.total_tokens)} tokens</span></button>`).join("") : `<div class="muted">暂无会话</div>`;
    if (nextSessionId) sessionList.insertAdjacentHTML("beforeend", `<div class="toolbar nested"><button id="agent-session-more" class="ghost">加载更多</button></div>`);
    sessionList.querySelectorAll("[data-session]").forEach((button) => button.onclick = () => showSession(button.dataset.session));
    const more = sessionList.querySelector("#agent-session-more");
    if (more) more.onclick = () => refreshSessions(nextSessionId);
  }

  async function refreshSessions(afterSessionId = null) {
    try {
      const body = await rpc.call("bot-agent", "sessions.list", { limit: 50, ...(afterSessionId ? { after_session_id: afterSessionId } : {}) });
      const page = body.local_agent_session_page || body;
      const items = page.items || [];
      sessions = afterSessionId ? sessions.concat(items) : items;
      nextSessionId = page.next_session_id || null;
      el.querySelector("#agent-provider-test").hidden = false;
      el.querySelector("#agent-session-refresh").hidden = false;
      renderSessions();
    } catch (error) {
      el.querySelector("#agent-provider-test").hidden = true;
      el.querySelector("#agent-session-refresh").hidden = true;
      sessionList.innerHTML = `<div class="muted">Agent 尚未启用</div>`;
    }
  }
  el.querySelector("#agent-provider-test").onclick = async (event) => {
    const button = event.currentTarget;
    const result = el.querySelector("#agent-provider-test-result");
    button.disabled = true;
    result.textContent = "正在测试…";
    try {
      await rpc.call("bot-agent", "provider.test", {});
      result.textContent = "模型可用";
    } catch (error) {
      result.textContent = errorText(error);
    } finally {
      button.disabled = false;
    }
  };
  el.querySelector("#agent-session-refresh").onclick = () => refreshSessions();
  await Promise.all([refresh(), refreshSessions()]);
}

export default { id: "bot-agent" };
