import { createWebShellRuntime } from "./shared/web-shell.js";
import { mountConsole, loadConsoleOptions, applyConsoleTheme } from "./index.js";

applyConsoleTheme();
const protocol = location.protocol === "https:" ? "wss" : "ws";
const shell = createWebShellRuntime({
  bridgeUrl: `${protocol}://${location.host}/ws`,
  authToken: "local-dev",
  capabilities: ["runtime.read", "runtime.write", "*"],
});
await shell.connect();
const options = await loadConsoleOptions();
mountConsole(document.getElementById("app"), shell.bridge, options);
window.addEventListener("pagehide", () => shell.dispose(), { once: true });
