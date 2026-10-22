import { MkOverlayHost, installMutsukiUi, uiMessages } from "@mutsuki/ui";
import { createI18n, provideI18n, provideMutsukiRuntime } from "@mutsuki/web-sdk";
import { createFakeBridge, type FakeBridge } from "@mutsuki/web-sdk/testing";
import { mount } from "@vue/test-utils";
import { defineComponent, h, type Component } from "vue";

/** Mounts a page the way the shell does: runtime, i18n and the overlay host provided. */
export function mountPage(
  component: Component,
  options: { bridge?: FakeBridge; props?: Record<string, unknown>; capabilities?: string[] } = {},
) {
  const bridge = options.bridge ?? createFakeBridge();
  const Host = defineComponent({
    setup: () => () => h("div", [h(component, options.props), h(MkOverlayHost)]),
  });
  const wrapper = mount(Host, {
    attachTo: document.body,
    global: {
      plugins: [
        {
          install(app) {
            const capabilities = new Set(options.capabilities ?? ["*"]);
            provideMutsukiRuntime(app, {
              client: bridge,
              session: { capabilities: () => capabilities, login: async () => true, logout: async () => {} },
            });
            provideI18n(app, createI18n().extend("ui", uiMessages));
            installMutsukiUi(app);
          },
        },
      ],
    },
  });
  return { wrapper, bridge };
}
