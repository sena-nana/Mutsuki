import type { Disposable, PageComponent } from "@mutsuki/web-sdk";
import { defineComponent, h, onBeforeUnmount, onMounted, ref, type Component } from "vue";

/** A page registered as a plain `{ mount }` object (hand-written assets before W6–W10). */
export function isLegacyPage(component: unknown): component is PageComponent {
  if (!component || typeof component !== "object") return false;
  const candidate = component as Record<string, unknown>;
  return (
    typeof candidate.mount === "function" &&
    !("setup" in candidate) &&
    !("render" in candidate) &&
    !("template" in candidate)
  );
}

/**
 * Wraps a vanilla page as a Vue component. Its DOM lives in one element the shell never
 * touches; mount errors (sync or async) reach the enclosing `ExtensionBoundary`. Removed in W11.
 */
export function legacyMount(page: PageComponent): Component {
  return defineComponent({
    name: "LegacyPage",
    setup() {
      const host = ref<HTMLElement | null>(null);
      let mounted: Disposable | void | null = null;
      let alive = true;
      onMounted(async () => {
        const result = await page.mount(host.value!);
        if (alive) mounted = result;
        else result?.dispose();
      });
      onBeforeUnmount(() => {
        alive = false;
        mounted?.dispose();
        mounted = null;
      });
      return () => h("div", { ref: host, class: "page-body legacy-page" });
    },
  });
}
