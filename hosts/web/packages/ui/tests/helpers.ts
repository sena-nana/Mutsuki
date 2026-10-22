import { configureAxe } from "vitest-axe";

/** happy-dom has no layout, so contrast cannot be computed; every other rule runs. */
export const axe = configureAxe({ rules: { "color-contrast": { enabled: false } } });

export async function settle(): Promise<void> {
  for (let index = 0; index < 4; index += 1) await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
}
