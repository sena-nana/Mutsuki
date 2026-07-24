import { SimpleRpc, mountConsole, loadConsoleOptions, applyConsoleTheme } from "./index.js";

applyConsoleTheme();
window.__MUTSUKI_CONSOLE__ = { includeConfig: true };
const params = new URLSearchParams(location.search);
const page = params.get("page") || "overview";
const proto = location.protocol === "https:" ? "wss" : "ws";

if (page === "config") {
  const { SimpleRpc: ConfigRpc, mountConfigConsole } = await import("./config/index.js");
  const rpc = new ConfigRpc(`${proto}://${location.host}/ws`);
  await rpc.connect();
  mountConfigConsole(document.getElementById("app"), rpc);
} else {
  const rpc = new SimpleRpc(`${proto}://${location.host}/ws`, {
    capabilities: ["runtime.read", "runtime.write", "*"],
  });
  await rpc.connect();
  const options = await loadConsoleOptions();
  mountConsole(document.getElementById("app"), rpc, {
    includeConfig: true,
    ...options,
  });
}
