import { SimpleRpc, mountConsole, loadConsoleOptions, applyConsoleTheme } from "./index.js";

applyConsoleTheme();
const proto = location.protocol === "https:" ? "wss" : "ws";
const rpc = new SimpleRpc(`${proto}://${location.host}/ws`, {
  capabilities: ["runtime.read", "runtime.write", "*"],
});
await rpc.connect();
const options = await loadConsoleOptions();
mountConsole(document.getElementById("app"), rpc, options);
