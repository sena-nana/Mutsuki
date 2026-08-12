const ALL = ["*"];

function esc(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function errorText(error) {
  if (error && typeof error === "object" && error.message) return error.message;
  return String(error);
}

export async function mountAgentConnectionsPanel(el, rpc) {
  el.innerHTML = `<div class="card"><h2>Agent 连接</h2><p class="muted">连接配置由 Agent owner 解释；Bot 仅引用 connection_id。</p><div id="agent-connection-list"></div></div>
    <div class="card"><h3>测试并应用</h3><div class="field"><label>连接配置 JSON</label><textarea id="agent-connection-json" rows="12">{\n  "connection_id": "primary",\n  "connector_id": "mutsuki.agent.connector.link.local",\n  "enabled": true,\n  "config": {}\n}</textarea></div><div class="toolbar nested"><button id="agent-test">测试连接</button><button id="agent-save">验证并切换</button></div><div id="agent-connection-result" class="muted"></div></div>`;
  let revision = 0;
  const list = el.querySelector("#agent-connection-list");
  const result = el.querySelector("#agent-connection-result");

  async function refresh() {
    const body = await rpc.call("bot-agent", "connections.snapshot", { capabilities: ALL });
    const snapshot = body.snapshot || body;
    revision = snapshot.revision || 0;
    const items = snapshot.connections || [];
    list.innerHTML = items.length ? items.map((item) => `<div class="tree-item row-item"><div><strong>${esc(item.connection_id)}</strong><div class="muted">${esc(item.connector_id)} · generation ${esc(item.generation)}</div></div><div class="row-actions"><span class="pill ${item.state === "healthy" ? "ok" : "warn"}">${esc(item.state)}</span><button class="ghost" data-reconnect="${esc(item.connection_id)}">重连</button></div></div>`).join("") : `<div class="muted">尚未配置连接</div>`;
    list.querySelectorAll("[data-reconnect]").forEach((button) => {
      button.onclick = async () => {
        try {
          await rpc.call("bot-agent", "connections.reconnect", { capabilities: ALL, expected_revision: revision, connection_id: button.dataset.reconnect });
          await refresh();
        } catch (error) { result.textContent = errorText(error); }
      };
    });
  }

  function config() { return JSON.parse(el.querySelector("#agent-connection-json").value); }
  el.querySelector("#agent-test").onclick = async () => {
    try { result.textContent = JSON.stringify(await rpc.call("bot-agent", "connections.test", { capabilities: ALL, config: config() }), null, 2); }
    catch (error) { result.textContent = errorText(error); }
  };
  el.querySelector("#agent-save").onclick = async () => {
    try {
      await rpc.call("bot-agent", "connections.upsert", { capabilities: ALL, expected_revision: revision, config: config() });
      result.textContent = "连接验证成功，已原子切换。";
      await refresh();
    } catch (error) { result.textContent = errorText(error); }
  };
  await refresh();
}

export default { id: "bot-agent" };
