import type { NavigationRegistration } from "@mutsuki/web-sdk";

/** One activity-bar entry. Only the product layout defines these; extensions cannot. */
export interface ConsoleActivity {
  id: string;
  label: string;
  icon: string;
  order: number;
  position: "top" | "bottom";
}

/** The product's information architecture: the only source of activities. */
export interface ConsoleLayout {
  activities: ConsoleActivity[];
}

export interface ShellFailure {
  extensionId: string;
  message: string;
  /** `load`: import/setup failed; `layout`: registration outside the layout; `render`: page crashed. */
  stage?: "load" | "layout" | "render";
}

/** Keeps navigation filed under layout activities; anything else is a layout failure. */
export function placeNavigation(
  layout: ConsoleLayout,
  items: NavigationRegistration[],
  ownerOf: (item: NavigationRegistration) => string,
): { placed: NavigationRegistration[]; failures: ShellFailure[] } {
  const known = new Set(layout.activities.map((activity) => activity.id));
  const placed: NavigationRegistration[] = [];
  const failures: ShellFailure[] = [];
  for (const item of items) {
    if (known.has(item.activityId)) {
      placed.push(item);
    } else {
      failures.push({
        extensionId: ownerOf(item),
        stage: "layout",
        message: `navigation ${item.id} targets activity ${item.activityId}, which the console layout does not define`,
      });
    }
  }
  return { placed, failures };
}
