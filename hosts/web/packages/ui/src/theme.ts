export type MutsukiTheme = "dark" | "light";

/** Apply Lilia/Mutsuki color scheme via `data-theme` (dark is default `:root`). */
export function applyTheme(theme: MutsukiTheme = "dark"): void {
  const root = document.documentElement;
  if (theme === "light") {
    root.dataset.theme = "light";
  } else {
    delete root.dataset.theme;
  }
  root.style.colorScheme = theme;
}

export function resolveTheme(preferred?: string | null): MutsukiTheme {
  if (preferred === "light" || preferred === "dark") return preferred;
  if (typeof window !== "undefined" && window.matchMedia) {
    if (window.matchMedia("(prefers-color-scheme: light)").matches) return "light";
  }
  return "dark";
}
