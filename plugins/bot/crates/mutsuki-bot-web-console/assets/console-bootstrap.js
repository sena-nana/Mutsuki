import { createWebShellRuntime } from "./shared/web-shell.js";
import { mountConsole, loadConsoleOptions, applyConsoleTheme } from "./index.js";

applyConsoleTheme();
const protocol = location.protocol === "https:" ? "wss" : "ws";
const app = document.getElementById("app");
let activeShell = null;

async function authenticate(authToken) {
  const shell = createWebShellRuntime({
    bridgeUrl: `${protocol}://${location.host}/ws`,
    authToken,
    capabilities: ["runtime.read", "runtime.write", "*"],
  });
  try {
    await shell.connect();
    const options = await loadConsoleOptions();
    activeShell = shell;
    app.replaceChildren();
    mountConsole(app, shell.bridge, options);
  } catch (error) {
    shell.dispose();
    throw error;
  }
}

function mountLogin(message = "") {
  app.innerHTML = `
    <main class="console-login">
      <form class="console-login__card">
        <p class="console-login__eyebrow">Mutsuki</p>
        <h1>登录管理台</h1>
        <p class="muted">输入产品配置的访问令牌。</p>
        <label>
          <span>访问令牌</span>
          <input name="token" type="password" autocomplete="current-password" required autofocus />
        </label>
        <p class="console-login__error" role="alert">${message}</p>
        <button type="submit">登录</button>
      </form>
    </main>`;
  const form = app.querySelector("form");
  const button = form.querySelector("button");
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const token = new FormData(form).get("token")?.toString() ?? "";
    if (!token) return;
    button.disabled = true;
    button.textContent = "正在登录…";
    try {
      await authenticate(token);
    } catch (_error) {
      mountLogin("访问令牌无效，请重试。");
    }
  });
}

mountLogin();
window.addEventListener("pagehide", () => activeShell?.dispose(), { once: true });
