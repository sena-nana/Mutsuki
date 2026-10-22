import { startConsole, type ConsoleOptions } from "@mutsuki/web-shell";

/** `console-options.json` is written by the host next to this module. */
async function main(root: HTMLElement): Promise<void> {
  const response = await fetch("./console-options.json", { cache: "no-store" });
  if (!response.ok) throw new Error(`console options unavailable (HTTP ${response.status})`);
  startConsole(root, (await response.json()) as ConsoleOptions);
}

const root = document.getElementById("app");
if (root) {
  main(root).catch((error: unknown) => {
    const message = document.createElement("p");
    message.setAttribute("role", "alert");
    message.className = "console-boot-error";
    message.textContent = `控制台加载失败：${error instanceof Error ? error.message : String(error)}`;
    root.replaceChildren(message);
  });
}
