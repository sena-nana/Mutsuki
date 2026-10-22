import type { NavigationRegistration, PageRegistration } from "@mutsuki/web-sdk";
import { computed, type ComputedRef } from "vue";

import type { ConsoleActivity, ConsoleLayout, ShellFailure } from "./layout.js";
import { placeNavigation } from "./layout.js";
import type { ShellState } from "./runtime.js";

/** What the shell renders: accessible pages, placed navigation, visible activities. */
export interface ShellModel {
  pages: Map<string, PageRegistration>;
  navByActivity: Map<string, NavigationRegistration[]>;
  activities: ConsoleActivity[];
  failures: ShellFailure[];
  firstRoute(): { activityId: string; pageId: string } | null;
  routeFor(activityId: string, pageId: string): string;
}

export function buildShellModel(
  state: ShellState,
  layout: ConsoleLayout,
  can: (capability?: string) => boolean,
  homePageId?: string,
): ShellModel {
  const pages = new Map(
    state.pages
      .list()
      .filter((page) => can(page.requiredCapability))
      .map((page) => [page.id, page]),
  );
  const { placed, failures } = placeNavigation(
    layout,
    state.navigation.list(),
    (item) => state.owners.get(item.id) ?? "shell",
  );
  const navByActivity = new Map<string, NavigationRegistration[]>();
  for (const item of placed) {
    if (!can(item.requiredCapability) || !pages.has(item.pageId)) continue;
    const list = navByActivity.get(item.activityId) ?? [];
    list.push(item);
    navByActivity.set(item.activityId, list);
  }
  for (const list of navByActivity.values()) {
    list.sort((a, b) => (a.order ?? 0) - (b.order ?? 0) || a.label.localeCompare(b.label, "zh-CN"));
  }
  const activities = [...layout.activities]
    .filter((activity) => navByActivity.get(activity.id)?.some((item) => !item.disabled))
    .sort((a, b) => a.order - b.order || a.label.localeCompare(b.label, "zh-CN"));
  const enabledIn = (activityId: string) =>
    navByActivity.get(activityId)?.find((item) => !item.disabled) ?? null;
  return {
    pages,
    navByActivity,
    activities,
    failures,
    firstRoute() {
      const preferred = homePageId
        ? placed.find((item) => item.pageId === homePageId && !item.disabled && pages.has(item.pageId))
        : undefined;
      const item = preferred ?? (activities[0] ? enabledIn(activities[0].id) : null);
      return item ? { activityId: item.activityId, pageId: item.pageId } : null;
    },
    routeFor(activityId, pageId) {
      return `/${encodeURIComponent(activityId)}/${encodeURIComponent(pageId)}`;
    },
  };
}

export function useActiveNavigation(
  model: ShellModel,
  route: ComputedRef<{ activityId: string; pageId: string } | null>,
) {
  return {
    activity: computed(() => model.activities.find((item) => item.id === route.value?.activityId)),
    items: computed(() => (route.value ? model.navByActivity.get(route.value.activityId) ?? [] : [])),
    page: computed(() => (route.value ? model.pages.get(route.value.pageId) : undefined)),
  };
}
