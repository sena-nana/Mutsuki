function message(error) {
  return error instanceof Error ? error.message : String(error ?? "操作失败");
}

async function mountUpgradePage(host, rpc) {
  host.innerHTML = `<div class="toolbar"><input type="search" placeholder="筛选模块" /><button type="button">检查更新</button></div><div class="upgrade-results"></div>`;
  const input = host.querySelector("input");
  const button = host.querySelector("button");
  const results = host.querySelector(".upgrade-results");

  async function refresh() {
    button.disabled = true;
    results.textContent = "正在检查…";
    try {
      const report = await rpc.read("upgrade", "check", { query: input.value.trim() });
      const modules = report.modules || [];
      results.innerHTML = "";
      if (!modules.length) {
        results.textContent = "没有可更新的模块";
        return;
      }
      for (const item of modules) {
        const card = document.createElement("article");
        card.className = "card";
        const title = document.createElement("h2");
        title.textContent = item.id;
        const detail = document.createElement("p");
        detail.className = "muted";
        detail.textContent = `${item.current_revision || "—"} → ${item.remote_revision || "—"}`;
        const plan = document.createElement("button");
        plan.type = "button";
        plan.textContent = "生成升级计划";
        plan.onclick = async () => {
          plan.disabled = true;
          try {
            const value = await rpc.read("upgrade", "plan", { module_id: item.id });
            const output = document.createElement("pre");
            output.className = "mono";
            output.textContent = value.cli_command || JSON.stringify(value.plan, null, 2);
            card.append(output);
          } catch (error) {
            detail.textContent = message(error);
          } finally {
            plan.disabled = false;
          }
        };
        card.append(title, detail, plan);
        results.append(card);
      }
    } catch (error) {
      results.textContent = message(error);
    } finally {
      button.disabled = false;
    }
  }
  button.onclick = refresh;
  input.onkeydown = (event) => { if (event.key === "Enter") void refresh(); };
  await refresh();
}

export default {
  id: "upgrade",
  setup(ctx) {
    ctx.pages.register({
      id: "upgrade.page", path: "/upgrade", title: "升级",
      component: { mount: (el) => mountUpgradePage(el, ctx.rpc) },
      requiredCapability: "runtime.read",
    });
    ctx.navigation.register({
      id: "upgrade.nav", activityId: "system", pageId: "upgrade.page", label: "升级", order: 90,
      requiredCapability: "runtime.read",
    });
  },
};
