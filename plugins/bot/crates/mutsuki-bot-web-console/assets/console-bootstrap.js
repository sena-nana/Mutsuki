import {
  createWebShellRuntime,
  createWebUiThemeController,
  mountWebShell,
} from "./shared/web-shell.js";

const theme = createWebUiThemeController();
const protocol = location.protocol === "https:" ? "wss" : "ws";
const app = document.getElementById("app");
let activeShell = null;
let activeMount = null;

async function loadConsoleOptions() {
  const response = await fetch("./console-options.json", { cache: "no-store" });
  if (!response.ok) throw new Error(`console options unavailable (${response.status})`);
  return response.json();
}

async function authenticate(authToken) {
  const shell = createWebShellRuntime({
    bridgeUrl: `${protocol}://${location.host}/ws`,
    authToken,
  });
  try {
    await shell.connect();
  } catch (error) {
    shell.dispose();
    throw new Error("auth_failed", { cause: error });
  }
  try {
    const options = await loadConsoleOptions();
    shell.configureActivities(options.activities);
    const hasConfig = (options.activities || []).some((item) => item.id === "config");
    shell.configureWebUiSettings(theme, hasConfig ? "config" : "settings");
    await shell.load(options.extensions);
    if (hasConfig && location.hash === "#/settings/config.page") {
      history.replaceState({}, "", "#/config/config.page");
    } else if (hasConfig && location.hash === "#/settings/webui.settings") {
      history.replaceState({}, "", "#/config/webui.settings");
    }
    activeShell = shell;
    app.replaceChildren();
    activeMount = mountWebShell(app, shell, { brand: "Mutsuki", homePageId: "overview.page" });
  } catch (error) {
    shell.dispose();
    console.error("console setup failed", error);
    throw new Error("setup_failed", { cause: error });
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
        <p class="console-login__description">输入启动时设置的管理口令。</p>
        <label class="console-login__field">
          <span>管理口令</span>
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
    } catch (error) {
      mountLogin(error?.message === "auth_failed" ? "口令不正确，请重试。" : "控制台加载失败，请重试。");
    }
  });
}

mountLogin();
window.addEventListener("pagehide", () => {
  activeMount?.dispose();
  activeShell?.dispose();
  theme.dispose();
}, { once: true });
