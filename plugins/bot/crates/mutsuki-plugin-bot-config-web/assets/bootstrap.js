import { SimpleRpc, mountConfigConsole, applyConsoleTheme } from "./index.js";

applyConsoleTheme();
const proto = location.protocol === "https:" ? "wss" : "ws";
const rpc = new SimpleRpc(`${proto}://${location.host}/ws`);
await rpc.connect();
mountConfigConsole(document.getElementById("app"), rpc);
