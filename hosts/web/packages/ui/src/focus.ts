import { nextTick, onBeforeUnmount, watch, type Ref } from "vue";

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export function focusableIn(root: HTMLElement): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
    (element) => !element.hasAttribute("inert") && element.getAttribute("aria-hidden") !== "true",
  );
}

/**
 * Modal focus handling: on open, focus moves into `panel` (the first focusable, or the
 * panel itself); Tab cycles inside it; on close, focus returns where it was.
 */
export function useFocusTrap(open: Ref<boolean>, panel: Ref<HTMLElement | null>): {
  onKeydown(event: KeyboardEvent): void;
} {
  let restore: HTMLElement | null = null;
  watch(
    open,
    async (isOpen) => {
      if (isOpen) {
        restore = (document.activeElement as HTMLElement | null) ?? null;
        await nextTick();
        const root = panel.value;
        if (!root) return;
        (root.querySelector<HTMLElement>("[autofocus]") ?? focusableIn(root)[0] ?? root).focus();
      } else if (restore) {
        const target = restore;
        restore = null;
        await nextTick();
        target.focus?.();
      }
    },
    { immediate: true },
  );
  onBeforeUnmount(() => restore?.focus?.());
  return {
    onKeydown(event) {
      if (event.key !== "Tab" || !panel.value) return;
      const items = focusableIn(panel.value);
      if (!items.length) {
        event.preventDefault();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    },
  };
}
