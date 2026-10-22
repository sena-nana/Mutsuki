import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import { init, parse } from "es-module-lexer";

/** Runtime modules the shell provides through its import map. Extensions never bundle them. */
export const SHARED_EXTERNALS = [
  "vue",
  "vue-router",
  "pinia",
  "@mutsuki/web-sdk",
  "@mutsuki/ui",
] as const;

export interface BundleBudgets {
  /** Largest allowed JavaScript file, in bytes. */
  jsBytes: number;
  /** Largest allowed CSS file, in bytes. */
  cssBytes: number;
}

export const DEFAULT_BUDGETS: BundleBudgets = {
  jsBytes: 200 * 1024,
  cssBytes: 40 * 1024,
};

export interface CheckOptions {
  /** Bare specifiers that may stay unresolved (served by the import map). */
  allowedImports?: readonly string[];
  budgets?: Partial<BundleBudgets>;
  /** Shared runtime builds legitimately contain Vue; extensions never do. */
  allowVueRuntime?: boolean;
  /** Raw colors are allowed only in the design system that defines the tokens. */
  allowHexColors?: boolean;
}

export interface CheckViolation {
  file: string;
  rule:
    | "bare_import"
    | "node_import"
    | "vue_runtime_bundled"
    | "js_budget"
    | "css_budget"
    | "css_hex_color";
  detail: string;
}

/**
 * Markers that only exist inside Vue's own runtime. Compiled SFCs reference these
 * through imports from "vue", so finding the literal means the runtime was inlined.
 */
const VUE_RUNTIME_MARKERS = ["__v_isVNode", "__v_isReadonly", "__v_raw"];

/** `#rgb`, `#rrggbb`, `#rrggbbaa` used as a declaration value (not an id selector). */
const HEX_COLOR_DECLARATION = /:[^;{}]*?#(?:[0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})\b/i;

export async function walkFiles(root: string): Promise<string[]> {
  const out: string[] = [];
  async function walk(dir: string) {
    for (const entry of await readdir(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) await walk(full);
      else out.push(full);
    }
  }
  await walk(root);
  return out.sort();
}

function isRelative(specifier: string): boolean {
  return (
    specifier.startsWith("./") ||
    specifier.startsWith("../") ||
    specifier.startsWith("/")
  );
}

/** Static and literal dynamic import specifiers of one ES module. */
export async function importSpecifiers(source: string): Promise<string[]> {
  await init();
  const [imports] = parse(source);
  return imports
    .filter((entry) => entry.type !== "dynamic" || !entry.glob)
    .map((entry) => entry.specifier)
    .filter((specifier): specifier is string => typeof specifier === "string");
}

/** Inspects one build output directory; an empty result means the bundle is shippable. */
export async function checkBundle(
  outDir: string,
  options: CheckOptions = {},
): Promise<CheckViolation[]> {
  const allowed = new Set(options.allowedImports ?? SHARED_EXTERNALS);
  const budgets = { ...DEFAULT_BUDGETS, ...options.budgets };
  const violations: CheckViolation[] = [];
  for (const file of await walkFiles(outDir)) {
    const relative = path.relative(outDir, file).replaceAll("\\", "/");
    const bytes = await readFile(file);
    if (file.endsWith(".js") || file.endsWith(".mjs")) {
      if (bytes.byteLength > budgets.jsBytes) {
        violations.push({
          file: relative,
          rule: "js_budget",
          detail: `${bytes.byteLength} bytes exceeds ${budgets.jsBytes}`,
        });
      }
      const source = bytes.toString("utf8");
      for (const specifier of await importSpecifiers(source)) {
        if (specifier.startsWith("node:")) {
          violations.push({ file: relative, rule: "node_import", detail: specifier });
        } else if (!isRelative(specifier) && !allowed.has(specifier)) {
          violations.push({ file: relative, rule: "bare_import", detail: specifier });
        }
      }
      if (!options.allowVueRuntime) {
        const marker = VUE_RUNTIME_MARKERS.find((candidate) => source.includes(candidate));
        if (marker) {
          violations.push({
            file: relative,
            rule: "vue_runtime_bundled",
            detail: `contains Vue runtime marker ${marker}`,
          });
        }
      }
    } else if (file.endsWith(".css")) {
      if (bytes.byteLength > budgets.cssBytes) {
        violations.push({
          file: relative,
          rule: "css_budget",
          detail: `${bytes.byteLength} bytes exceeds ${budgets.cssBytes}`,
        });
      }
      if (!options.allowHexColors) {
        const css = bytes.toString("utf8").replace(/\/\*[\s\S]*?\*\//g, "");
        const match = css.match(HEX_COLOR_DECLARATION);
        if (match) {
          violations.push({
            file: relative,
            rule: "css_hex_color",
            detail: `raw color in "${match[0].trim()}"; use a Lilia/--mk-* token`,
          });
        }
      }
    }
  }
  return violations;
}

export function formatViolations(violations: CheckViolation[]): string {
  return violations.map((item) => `${item.file}: [${item.rule}] ${item.detail}`).join("\n");
}
