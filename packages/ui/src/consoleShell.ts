import { defineComponent, h, type PropType, type VNode } from "vue";

export type ConsoleNavItem = {
  id: string;
  label: string;
  href?: string;
  active?: boolean;
};

export type ConsoleShellSlots = {
  brand?: () => VNode | VNode[];
  nav?: () => VNode | VNode[];
  footer?: () => VNode | VNode[];
  header?: () => VNode | VNode[];
  default?: () => VNode | VNode[];
};

/** Lilia workspace chrome: secondary-panel nav + page-header main. */
export const ConsoleShell = defineComponent({
  name: "MutsukiConsoleShell",
  props: {
    brand: { type: String, default: "Mutsuki" },
    footer: { type: String, default: "Bot 控制台" },
    title: { type: String, default: "" },
    subtitle: { type: String, default: "" },
    navItems: {
      type: Array as PropType<ConsoleNavItem[]>,
      default: () => [],
    },
  },
  emits: ["navigate", "refresh"],
  setup(props, { emit, slots }) {
    return () =>
      h(
        "div",
        {
          class: "mutsuki-console lilia-workspace",
          "data-lilia-surface-mode": "solid",
          "data-lilia-surface-level": "base",
        },
        [
          h(
            "aside",
            {
              class: "lilia-workspace-region",
              "data-region": "navigation",
              "data-region-separator": "inline",
            },
            [
              h("div", { class: "secondary-panel" }, [
                h("div", { class: "secondary-panel__top" }, [
                  slots.brand?.() ?? h("div", { class: "brand" }, props.brand),
                ]),
                slots.nav?.() ??
                  h(
                    "nav",
                    {
                      class: "secondary-panel__body sb-section nav",
                      "aria-label": "控制台",
                    },
                    props.navItems.map((item) => {
                      const className = [
                        "sb-tree__row",
                        "lilia-interactive-item",
                        item.active ? "is-active" : undefined,
                      ];
                      const label = [h("span", { class: "sb-tree__name" }, item.label)];
                      if (item.href) {
                        return h(
                          "a",
                          {
                            class: className,
                            href: item.href,
                            "aria-current": item.active ? "page" : undefined,
                          },
                          label,
                        );
                      }
                      return h(
                        "button",
                        {
                          type: "button",
                          class: className,
                          "aria-current": item.active ? "page" : undefined,
                          onClick: () => emit("navigate", item.id),
                        },
                        label,
                      );
                    }),
                  ),
                slots.footer?.() ??
                  h("div", { class: "secondary-panel__footer sidebar-footer" }, props.footer),
              ]),
            ],
          ),
          h(
            "main",
            {
              class: "lilia-workspace-region",
              "data-region": "main",
            },
            [
              h("div", { class: "lilia-workspace-region__content page-scroll" }, [
                slots.header?.() ??
                  h("div", { class: "page-header" }, [
                    h("div", [
                      h("h1", props.title),
                      props.subtitle ? h("p", props.subtitle) : null,
                    ]),
                    h("div", { class: "page-actions" }, [
                      h(
                        "button",
                        {
                          type: "button",
                          class: "ghost",
                          onClick: () => emit("refresh"),
                        },
                        "刷新",
                      ),
                    ]),
                  ]),
                h("section", { class: "page-body", id: "content" }, slots.default?.() ?? []),
              ]),
            ],
          ),
        ],
      );
  },
});

/** Vanilla DOM factory matching ConsoleShell markup. */
export function createConsoleShellElement(options: {
  brand?: string;
  footer?: string;
  title?: string;
  subtitle?: string;
  showRefresh?: boolean;
}): HTMLElement {
  const root = document.createElement("div");
  root.className = "mutsuki-console lilia-workspace";
  root.dataset.liliaSurfaceMode = "solid";
  root.dataset.liliaSurfaceLevel = "base";
  root.innerHTML = `
    <aside class="lilia-workspace-region" data-region="navigation" data-region-separator="inline">
      <div class="secondary-panel">
        <div class="secondary-panel__top">
          <div class="brand">${escapeHtml(options.brand ?? "Mutsuki")}</div>
        </div>
        <nav class="secondary-panel__body sb-section nav" aria-label="控制台"></nav>
        <div class="secondary-panel__footer sidebar-footer">${escapeHtml(options.footer ?? "Bot 控制台")}</div>
      </div>
    </aside>
    <main class="lilia-workspace-region" data-region="main">
      <div class="lilia-workspace-region__content page-scroll">
        <div class="page-header">
          <div>
            <h1 id="page-title">${escapeHtml(options.title ?? "")}</h1>
            <p id="page-subtitle">${escapeHtml(options.subtitle ?? "")}</p>
          </div>
          ${
            options.showRefresh === false
              ? ""
              : `<div class="page-actions"><button type="button" id="refresh" class="ghost">刷新</button></div>`
          }
        </div>
        <section id="content" class="page-body"></section>
      </div>
    </main>
  `;
  return root;
}

function escapeHtml(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}
