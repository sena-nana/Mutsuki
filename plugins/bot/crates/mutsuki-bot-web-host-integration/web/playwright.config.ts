import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig, devices } from "@playwright/test";

const here = path.dirname(fileURLToPath(import.meta.url));
const listen = process.env.MUTSUKI_CONSOLE_FIXTURE_LISTEN ?? "127.0.0.1:4317";

/**
 * Console smoke against `examples/embedded_console_fixture.rs` (fixture control plane, demo
 * config provider, simulated sandbox). Real accounts are never involved.
 */
export default defineConfig({
  testDir: "e2e",
  timeout: 30_000,
  expect: { timeout: 10_000 },
  retries: process.env.CI ? 1 : 0,
  workers: 1,
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : "list",
  use: {
    baseURL: `http://${listen}`,
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: {
    command:
      "cargo run --locked --quiet -p mutsuki-bot-web-host-integration --example embedded_console_fixture",
    cwd: path.resolve(here, "../../../../.."),
    url: `http://${listen}/health`,
    env: { MUTSUKI_CONSOLE_FIXTURE_LISTEN: listen },
    timeout: 15 * 60_000,
    reuseExistingServer: !process.env.CI,
    stdout: "ignore",
  },
});
