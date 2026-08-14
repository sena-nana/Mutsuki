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
      <form class="console-login__card" aria-labelledby="console-login-title">
        <div class="console-login__brand">
          <span class="console-login__brand-mark" aria-hidden="true"></span>
          Mutsuki
        </div>
        <h1 id="console-login-title">登录管理台</h1>
        <p class="console-login__description">输入产品配置的访问令牌。</p>
        <label class="console-login__field">
          <span>访问令牌</span>
          <input
            name="token"
            type="password"
            autocomplete="current-password"
            aria-describedby="console-login-error"
            required
          />
        </label>
        <p id="console-login-error" class="console-login__error" role="alert"></p>
        <button class="console-login__submit" type="submit">登录</button>
      </form>
    </main>`;
  const form = app.querySelector("form");
  const input = form.querySelector("input");
  const button = form.querySelector("button");
  const error = form.querySelector(".console-login__error");
  error.textContent = message;
  input.setAttribute("aria-invalid", message ? "true" : "false");
  input.focus();
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    if (form.dataset.pending === "true") return;
    const token = new FormData(form).get("token")?.toString() ?? "";
    if (!token) return;
    form.dataset.pending = "true";
    form.setAttribute("aria-busy", "true");
    input.disabled = true;
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
