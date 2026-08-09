const ALL = ["*"];

function esc(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function lines(value) {
  return String(value || "").split(/[,\n]/).map((v) => v.trim()).filter(Boolean);
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

async function selectBotAgentConnection(rpc, connectionId) {
  const provider_id = "mutsuki.plugin.bot.agent";
  const context = { scope: "plugin_instance", plugin_instance_id: "default" };
  const snapshot = await rpc.call("config", "snapshot.read", { capabilities: ALL, provider_id, context });
  const candidate = structuredClone(snapshot.value);
  if (!candidate?.value) throw new Error("Bot Agent 配置快照格式无效");
  candidate.value.connection_id = { type: "string", value: connectionId };
  candidate.value.enabled = { type: "bool", value: true };
  await rpc.call("config", "apply", {
    capabilities: ALL,
    provider_id,
    context,
    request: { candidate, expected_revision: snapshot.revision?.value ?? snapshot.revision ?? 1, dry_run: false },
  });
}

export async function mountBotAgentRulesPanel(el, rpc) {
  el.innerHTML = `<div class="card"><h2>Bot Agent 规则</h2><p class="muted">只有未被高优先级 Handler 消费的消息才会评估这些规则。</p><div class="field"><label>健康连接</label><select id="rule-connection"><option value="">不修改 Bot Agent 连接</option></select></div><button id="rule-select-connection" class="ghost">启用并选择连接</button><div id="policy-rules"></div></div>
    <div class="card"><h3>新增或更新规则</h3><div class="field"><label>规则 ID</label><input id="rule-id" value="product-default" /></div><div class="field"><label>匹配条件 JSON（空对象为 Product 层）</label><textarea id="rule-match" rows="5">{}</textarea></div><div class="field"><label>触发预设</label><select id="rule-preset"><option value="disabled">停用</option><option value="all_unconsumed_messages">所有未消费消息</option><option value="mention_or_wake_word">提及或唤醒词</option></select></div><div class="field"><label>唤醒词（逗号或换行）</label><textarea id="rule-wake" rows="2"></textarea></div><div class="field"><label>私聊策略</label><select id="rule-direct"><option value="allow">允许</option><option value="deny">拒绝</option><option value="allowlisted">仅白名单</option></select></div><div class="field"><label>Allowlist</label><input id="rule-allow" /></div><div class="field"><label>Denylist</label><input id="rule-deny" /></div><div class="field"><label>Agent profile</label><input id="rule-profile" /></div><div class="field"><label>会话作用域</label><select id="rule-scope"><option value="shared_conversation">共享会话</option><option value="actor_in_conversation">按用户隔离</option></select></div><div class="toolbar nested"><label><input type="checkbox" id="rule-stt" /> STT</label><label><input type="checkbox" id="rule-tts" /> TTS</label><button id="rule-save">保存规则</button></div><div id="rule-result" class="muted"></div></div>`;
  let revision = 0;
  const rulesHost = el.querySelector("#policy-rules");
  const result = el.querySelector("#rule-result");

  async function refreshConnections() {
    try {
      const body = await rpc.call("bot-agent", "connections.snapshot", { capabilities: ALL });
      const healthy = (body.snapshot?.connections || []).filter((item) => item.state === "healthy");
      el.querySelector("#rule-connection").insertAdjacentHTML("beforeend", healthy.map((item) => `<option value="${esc(item.connection_id)}">${esc(item.connection_id)}</option>`).join(""));
    } catch { /* Connection owner is optional and its page is hidden independently. */ }
  }

  async function refreshRules() {
    const body = await rpc.call("bot-agent", "policies.snapshot", { capabilities: ALL });
    revision = body.revision || 0;
    rulesHost.innerHTML = (body.rules || []).length ? body.rules.map((rule) => `<div class="tree-item row-item"><div><strong>${esc(rule.rule_id)}</strong><div class="muted">revision ${esc(rule.revision)} · ${esc(JSON.stringify(rule.matcher))}</div></div><button class="ghost" data-delete="${esc(rule.rule_id)}">删除</button></div>`).join("") : `<div class="muted">尚无规则，Agent 保持禁用。</div>`;
    rulesHost.querySelectorAll("[data-delete]").forEach((button) => {
      button.onclick = async () => {
        try {
          await rpc.call("bot-agent", "policies.delete", { capabilities: ALL, request: { actor_id: "web-console", expected_revision: revision, rule_id: button.dataset.delete } });
          await refreshRules();
        } catch (error) { result.textContent = errorText(error); }
      };
    });
  }

  el.querySelector("#rule-select-connection").onclick = async () => {
    const id = el.querySelector("#rule-connection").value;
    if (!id) return;
    try { await selectBotAgentConnection(rpc, id); result.textContent = `已选择健康连接 ${id}`; }
    catch (error) { result.textContent = `需要装配 Config Web：${errorText(error)}`; }
  };
  el.querySelector("#rule-save").onclick = async () => {
    try {
      const preset = el.querySelector("#rule-preset").value;
      const wakeWords = lines(el.querySelector("#rule-wake").value);
      const patch = {
        enabled: true,
        agent_enabled: preset !== "disabled",
        direct_message_policy: el.querySelector("#rule-direct").value,
        must_mention: preset === "mention_or_wake_word",
        wake_words: preset === "mention_or_wake_word" ? wakeWords : [],
        allowlist: lines(el.querySelector("#rule-allow").value),
        denylist: lines(el.querySelector("#rule-deny").value),
        session_scope: el.querySelector("#rule-scope").value,
        agent_runtime_profile_id: el.querySelector("#rule-profile").value || null,
        stt_enabled: el.querySelector("#rule-stt").checked,
        tts_enabled: el.querySelector("#rule-tts").checked,
      };
      await rpc.call("bot-agent", "policies.upsert", { capabilities: ALL, request: { actor_id: "web-console", expected_revision: revision, rule: { rule_id: el.querySelector("#rule-id").value.trim(), revision, matcher: JSON.parse(el.querySelector("#rule-match").value), patch } } });
      result.textContent = "规则已持久化。";
      await refreshRules();
    } catch (error) { result.textContent = errorText(error); }
  };
  await Promise.all([refreshConnections(), refreshRules()]);
}

export default { id: "bot-agent" };
