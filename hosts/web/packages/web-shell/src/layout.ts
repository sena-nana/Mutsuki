import type { NavigationRegistration } from "@mutsuki/web-sdk";

/** One activity-bar entry. Only the product layout defines these; extensions cannot. */
export interface ConsoleActivity {
  id: string;
  label: string;
  icon: string;
  order: number;
  position: "top" | "bottom";
}

/**
 * The product's information architecture. `placements` moves navigation registered by
 * not-yet-migrated extensions: keys are a navigation id or a legacy activity id, values an
 * activity of this layout. It shrinks as extensions migrate and disappears in W11.
 */
export interface ConsoleLayout {
  activities: ConsoleActivity[];
  placements?: Record<string, string>;
}

export interface ShellFailure {
  extensionId: string;
  message: string;
  /** `load`: import/setup failed; `layout`: registration outside the layout; `render`: page crashed. */
  stage?: "load" | "layout" | "render";
}

/** Places navigation into layout activities; anything left over is a layout failure. */
export function placeNavigation(
  layout: ConsoleLayout,
  items: NavigationRegistration[],
  ownerOf: (item: NavigationRegistration) => string,
): { placed: NavigationRegistration[]; failures: ShellFailure[] } {
  const known = new Set(layout.activities.map((activity) => activity.id));
  const placements = layout.placements ?? {};
  const placed: NavigationRegistration[] = [];
  const failures: ShellFailure[] = [];
  for (const item of items) {
    const activityId = placements[item.id] ?? placements[item.activityId] ?? item.activityId;
    if (known.has(activityId)) {
      placed.push(activityId === item.activityId ? item : { ...item, activityId });
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
