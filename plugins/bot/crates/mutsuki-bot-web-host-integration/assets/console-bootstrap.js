import {
  createWebShellRuntime,
  createWebUiThemeController,
  mountWebShell,
} from "@mutsuki/web-shell";

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

/** Trades the console token for an HttpOnly session cookie; the token is not kept. */
async function login(token) {
  const response = await fetch("./auth/session", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ token }),
    credentials: "same-origin",
  });
  if (response.status === 401) throw new Error("auth_failed");
  if (!response.ok) throw new Error(`login_failed_${response.status}`);
}

/** Connects with the session cookie alone, so a reload resumes without the token. */
async function authenticate() {
  const shell = createWebShellRuntime({
    bridgeUrl: `${protocol}://${location.host}/ws`,
  });
  try {
    await shell.connect();
  } catch (error) {
    shell.dispose();
    throw new Error(error?.code === "auth_failed" ? "auth_failed" : "connect_failed", {
      cause: error,
    });
  }
  try {
    const options = await loadConsoleOptions();
    shell.configureActivities(options.activities);
    const hasPlugins = (options.activities || []).some((item) => item.id === "plugins");
    shell.configureWebUiSettings(theme, hasPlugins ? "plugins" : "settings");
    await shell.load(options.extensions);
    if (hasPlugins && location.hash.startsWith("#/config/")) {
      history.replaceState({}, "", `#/plugins/${location.hash.slice("#/config/".length)}`);
    } else if (hasPlugins && location.hash.startsWith("#/settings/")) {
      history.replaceState({}, "", `#/plugins/${location.hash.slice("#/settings/".length)}`);
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
      await login(token);
      await authenticate();
    } catch (error) {
      mountLogin(error?.message === "auth_failed" ? "口令不正确，请重试。" : "控制台加载失败，请重试。");
    }
  });
}

authenticate().catch((error) => {
  // No live session cookie is the normal first visit, not an error worth showing.
  mountLogin(error?.message === "auth_failed" ? "" : "控制台加载失败，请重试。");
});
window.addEventListener("pagehide", () => {
  activeMount?.dispose();
  activeShell?.dispose();
  theme.dispose();
}, { once: true });
