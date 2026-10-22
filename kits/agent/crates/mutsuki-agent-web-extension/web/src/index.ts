import { RpcError, defineExtension } from "@mutsuki/web-sdk";

import AgentModelsCard from "./AgentModelsCard.vue";
import ApprovalsPage from "./ApprovalsPage.vue";
import ConnectionsPage from "./ConnectionsPage.vue";
import SessionsPage from "./SessionsPage.vue";

const OWNERS = ["mutsuki.agent.runtime.local", "mutsuki.plugin.bot.agent"];

/**
 * Agent console in the `agent` activity. The connection and session services are optional on
 * the host, so each surface is registered only when its RPCs answer (a capability denial still
 * registers: the page then explains the missing permission).
 */
export default defineExtension({
  id: "mutsuki.agent.console",
  async setup(ctx) {
    const available = async (method: string, params: object) => {
      try {
        await ctx.rpc.read("bot-agent", method, params as never);
        return true;
      } catch (error) {
        return error instanceof RpcError && error.code === "capability_denied";
      }
    };
    const [connections, sessions] = await Promise.all([
      available("connections.snapshot", {}),
      available("sessions.list", { limit: 1 }),
    ]);
    const page = (id: string, path: string, title: string, order: number, component: object, capability: string) => {
      ctx.pages.register({ id, path, title, component, requiredCapability: capability, pluginIds: OWNERS });
      ctx.navigation.register({ id: `${id}.nav`, activityId: "agent", pageId: id, label: title, order, requiredCapability: capability });
    };
    if (sessions) {
      page("bot-agent.page", "/agent", "会话", 10, SessionsPage, "agent.session.read");
      page("agent.approvals", "/agent/approvals", "审批", 20, ApprovalsPage, "agent.session.read");
    }
    if (connections) {
      page("agent.connections", "/agent/connections", "连接", 30, ConnectionsPage, "agent.connection.read");
      ctx.slots.register({
        id: "bot-agent.overview.cards",
        slot: "overview.cards",
        pluginIds: OWNERS,
        requiredCapability: "agent.connection.read",
        component: AgentModelsCard,
      });
    }
  },
});
