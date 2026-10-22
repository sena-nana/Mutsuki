import { RpcError, callRpc, defineExtension, type DefinedExtensionContext, type ExtensionContext } from "@mutsuki/web-sdk";
import { defineComponent, h, type Component } from "vue";

import { configApi, controlPlugins, type PluginEntry } from "./contract";
import { BUILTIN_FORMATS, BUILTIN_KINDS } from "./fields";
import PluginHub from "./PluginHub.vue";
import { createFieldRenderers, type FieldRenderers } from "./renderers";

export { default as SchemaForm } from "./SchemaForm.vue";
export type { FieldRendererProps } from "./renderers";

const FIELD_SLOT = "config.field:";

/** Built-in renderers plus every `config.field:<format>` contribution, read when a hub opens. */
function fieldRenderers(ctx: DefinedExtensionContext): FieldRenderers {
  const contributed = Object.fromEntries(
    ctx.slots
      .list()
      .filter((item) => item.slot.startsWith(FIELD_SLOT))
      .map((item) => [item.slot.slice(FIELD_SLOT.length), item.component as Component]),
  );
  return createFieldRenderers(BUILTIN_KINDS, { ...BUILTIN_FORMATS, ...contributed });
}

/**
 * Loaded plugins come from the Control API. It is optional for the hubs: when it is missing or
 * fails, product-named providers still get their hubs and only the "loaded" group is skipped.
 */
async function loadedPlugins(ctx: DefinedExtensionContext): Promise<PluginEntry[]> {
  try {
    return (await callRpc(ctx.rpc, controlPlugins.plugin_list)).plugins;
  } catch {
    return [];
  }
}

/** The provider's title, or `null` when the id names a plugin without a config schema. */
async function schemaTitle(ctx: DefinedExtensionContext, providerId: string): Promise<string | null> {
  try {
    const schema = await callRpc(ctx.rpc, configApi["schema.get"], { provider_id: providerId });
    return schema.title.zh_cn || schema.title.default || providerId;
  } catch (error) {
    if (error instanceof RpcError && error.code !== "capability_denied") return null;
    throw error;
  }
}

/**
 * Plugin hubs in the `plugins` activity: one per product-declared navigation item (in its group),
 * then one per loaded plugin the product did not name. Each hub shows the plugin's cards, pages and
 * config form; the `plugin.home` slot lets the shell open hubs for plugins found only through
 * their pages or cards.
 */
export default defineExtension({
  id: "mutsuki.std.config",
  async setup(ctx) {
    const registries: Pick<ExtensionContext, "pages" | "navigation"> = { pages: ctx.pages, navigation: ctx.navigation };
    const hub = (pluginId: string, hasSchema: boolean) =>
      defineComponent({
        name: "PluginHubPage",
        setup() {
          const renderers = fieldRenderers(ctx);
          return () => h(PluginHub, { pluginId, hasSchema, slots: ctx.slots, registries, renderers });
        },
      });
    const owns = (pluginId: string) =>
      ctx.pages.list().some((page) => page.id !== pluginId && (page.pluginId === pluginId || page.pluginIds?.includes(pluginId))) ||
      ctx.slots
        .list()
        .some((slot) => slot.slot === "overview.cards" && (slot.pluginId === pluginId || slot.pluginIds?.includes(pluginId)));

    ctx.slots.register({
      id: "config.plugin-home",
      slot: "plugin.home",
      component: defineComponent({
        name: "PluginHome",
        props: { pluginId: { type: String, required: true } },
        setup(props) {
          const renderers = fieldRenderers(ctx);
          return () => h(PluginHub, { pluginId: props.pluginId, hasSchema: false, slots: ctx.slots, registries, renderers });
        },
      }),
    });

    const [groups, plugins] = await Promise.all([callRpc(ctx.rpc, configApi["navigation.list"]), loadedPlugins(ctx)]);
    const loaded = new Set(plugins.filter((plugin) => plugin.configured || plugin.active_deployment).map((plugin) => plugin.plugin_id));
    const entries = groups.flatMap((group) => group.items.map((item) => ({ group: group.label, item })));
    const titles = await Promise.all(entries.map(({ item }) => schemaTitle(ctx, item.provider_id)));

    const covered = new Set<string>();
    const register = (pluginId: string, title: string, group: string | undefined, order: number, hasSchema: boolean) => {
      const requiredCapability = hasSchema ? "config.schema.read" : "runtime.read";
      ctx.pages.register({ id: pluginId, path: `/plugins/${pluginId}`, title, pluginId, requiredCapability, component: hub(pluginId, hasSchema) });
      ctx.navigation.register({
        id: `${pluginId}.nav`,
        activityId: "plugins",
        pageId: pluginId,
        label: title,
        group,
        order,
        requiredCapability,
        // A schema-less plugin with no pages or cards has nothing to show.
        disabled: !hasSchema && !owns(pluginId) ? true : undefined,
      });
      covered.add(pluginId);
    };

    entries.forEach(({ group, item }, index) => {
      const title = titles[index];
      const hasSchema = title !== null;
      // Product-named plugins without a schema only appear once they are loaded.
      if (covered.has(item.provider_id) || (!hasSchema && !loaded.has(item.provider_id))) return;
      register(item.provider_id, item.label || title || item.provider_id, group, index, hasSchema);
    });
    let order = entries.length;
    for (const pluginId of loaded) {
      if (covered.has(pluginId)) continue;
      register(pluginId, pluginId, "已加载", order, false);
      order += 1;
    }
  },
});
