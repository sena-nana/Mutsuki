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

/**
 * Lilia Workspace-shaped console chrome: navigation region + main region.
 * Uses official `.lilia-workspace` / `.lilia-workspace-region` classes.
 */
export const ConsoleShell = defineComponent({
  name: "MutsukiConsoleShell",
  props: {
    brand: { type: String, default: "Mutsuki" },
    footer: { type: String, default: "bot console" },
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
              h("div", { class: "lilia-workspace-region__content" }, [
                slots.brand?.() ?? h("div", { class: "brand" }, props.brand),
                slots.nav?.() ??
                  h(
                    "nav",
                    { class: "nav", "aria-label": "Console" },
                    props.navItems.map((item) =>
                      item.href
                        ? h(
                            "a",
                            {
                              class: [
                                "nav-item",
                                item.active ? "active" : undefined,
                              ],
                              href: item.href,
                              "aria-current": item.active ? "page" : undefined,
                            },
                            item.label,
                          )
                        : h(
                            "button",
                            {
                              type: "button",
                              class: [
                                "nav-item",
                                item.active ? "active" : undefined,
                              ],
                              "aria-current": item.active ? "page" : undefined,
                              onClick: () => emit("navigate", item.id),
                            },
                            item.label,
                          ),
                    ),
                  ),
                slots.footer?.() ??
                  h("div", { class: "sidebar-footer" }, props.footer),
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
              h("div", { class: "lilia-workspace-region__content" }, [
                slots.header?.() ??
                  h("header", { class: "workspace-header" }, [
                    h("div", { class: "header-row" }, [
                      h("div", [
                        h("h1", props.title),
                        props.subtitle
                          ? h("p", props.subtitle)
                          : null,
                      ]),
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
                h(
                  "section",
                  { class: "workspace-content", id: "content" },
                  slots.default?.() ?? [],
                ),
              ]),
            ],
          ),
        ],
      );
  },
});

/** Vanilla DOM factory matching ConsoleShell markup (for non-Vue console pages). */
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
      <div class="lilia-workspace-region__content">
        <div class="brand">${escapeHtml(options.brand ?? "Mutsuki")}</div>
        <nav class="nav" aria-label="Console"></nav>
        <div class="sidebar-footer">${escapeHtml(options.footer ?? "bot console")}</div>
      </div>
    </aside>
    <main class="lilia-workspace-region" data-region="main">
      <div class="lilia-workspace-region__content">
        <header class="workspace-header">
          <div class="header-row">
            <div>
              <h1 id="page-title">${escapeHtml(options.title ?? "")}</h1>
              <p id="page-subtitle">${escapeHtml(options.subtitle ?? "")}</p>
            </div>
            ${
              options.showRefresh === false
                ? ""
                : `<button type="button" id="refresh" class="ghost">刷新</button>`
            }
          </div>
        </header>
        <section id="content" class="workspace-content"></section>
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
