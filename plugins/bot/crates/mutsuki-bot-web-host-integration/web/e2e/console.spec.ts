import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

/** Test token hard-coded in `examples/embedded_console_fixture.rs`; not a real credential. */
const FIXTURE_TOKEN = "embedded-console-fixture";

function collectErrors(page: Page): string[] {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(`pageerror: ${error.message}`));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`);
  });
  return errors;
}

async function login(page: Page): Promise<void> {
  await page.goto("/");
  await page.getByLabel("管理口令").fill(FIXTURE_TOKEN);
  await page.getByRole("button", { name: "登录" }).click();
  await expect(page.getByRole("navigation", { name: "工作域" })).toBeVisible();
}

/** Every `#/activity/page` route reachable from the activity bar and context navigation. */
async function allRoutes(page: Page): Promise<string[]> {
  const routes = new Set<string>();
  const activities = await page
    .getByRole("navigation", { name: "工作域" })
    .getByRole("link")
    .evaluateAll((links) => links.map((link) => link.getAttribute("href") ?? ""));
  for (const activity of activities) {
    await page.goto(`/${activity}`);
    await page.waitForURL((url) => url.hash.split("/").length === 3);
    routes.add(new URL(page.url()).hash);
    const pages = await page
      .locator(".console-context__nav a")
      .evaluateAll((links) => links.map((link) => link.getAttribute("href") ?? ""));
    for (const href of pages) routes.add(href);
  }
  return [...routes].filter(Boolean).sort();
}

test("wrong token is refused without leaving the login", async ({ page }) => {
  await page.goto("/");
  await page.getByLabel("管理口令").fill("not-the-token");
  await page.getByRole("button", { name: "登录" }).click();
  await expect(page.getByRole("alert")).toHaveText("口令不正确，请重试。");
  await expect(page.getByRole("navigation", { name: "工作域" })).toHaveCount(0);
});

test("login lands on the home page", async ({ page }) => {
  await login(page);
  await expect(page).toHaveURL(/#\/home\/overview\.page$/);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("概览");
  await expect(page.locator(".console-page-content .metric-grid")).toBeVisible();
});

test("an unknown route falls back to the home page", async ({ page }) => {
  await login(page);
  await page.goto("/#/nowhere/missing.page");
  await expect(page).toHaveURL(/#\/home\/overview\.page$/);
});

test("every page mounts without boundary or console errors", async ({ page }) => {
  const errors = collectErrors(page);
  await login(page);
  const routes = await allRoutes(page);
  expect(routes.length).toBeGreaterThanOrEqual(8);
  for (const route of routes) {
    await page.goto(`/${route}`);
    const content = page.locator(".console-page-content");
    await expect(content).toBeVisible();
    await expect(content.locator(":scope > *").first()).toBeVisible();
    await page.waitForLoadState("networkidle");
    await expect(content.locator(".mk-state--error"), `boundary error on ${route}`).toHaveCount(0);
  }
  await expect(page.locator(".console-extension-retry"), "extension failures").toHaveCount(0);
  expect(errors).toEqual([]);
});

test("theme switches between light and dark", async ({ page }) => {
  await login(page);
  await page.goto("/#/settings/webui.settings");
  const theme = page.getByRole("radiogroup", { name: "主题" });
  await theme.getByRole("radio", { name: "浅色" }).click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await theme.getByRole("radio", { name: "深色" }).click();
  await expect(page.locator("html")).not.toHaveAttribute("data-theme", /.+/);
});

for (const colorScheme of ["light", "dark"] as const) {
  test(`shell chrome and Vue pages have no serious axe violations (${colorScheme})`, async ({ page }) => {
    await page.emulateMedia({ colorScheme });
    await page.goto("/");
    const loginScan = await new AxeBuilder({ page }).analyze();
    expect(serious(loginScan.violations)).toEqual([]);

    await login(page);
    for (const route of ["#/home/overview.page", "#/settings/webui.settings", "#/system/control.plugins"]) {
      await page.goto(`/${route}`);
      await page.waitForLoadState("networkidle");
      // Hand-written pages are migrated in W6–W10; until then only the shell around them counts.
      const scan = await new AxeBuilder({ page }).exclude(".legacy-page").analyze();
      expect(serious(scan.violations), route).toEqual([]);
    }
  });
}

test("a reload resumes the cookie session without the token", async ({ page }) => {
  await login(page);
  await page.reload();
  await expect(page.getByRole("navigation", { name: "工作域" })).toBeVisible();
  await expect(page.getByLabel("管理口令")).toHaveCount(0);
});

function serious(
  violations: Array<{ id: string; impact?: string | null; nodes: Array<{ target: unknown[] }> }>,
) {
  return violations
    .filter((violation) => violation.impact === "serious" || violation.impact === "critical")
    .map((violation) => `${violation.id}: ${violation.nodes.map((node) => node.target.join(" ")).join(", ")}`);
}
