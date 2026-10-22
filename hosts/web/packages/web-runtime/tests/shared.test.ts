import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, test } from "vitest";

import { importSpecifiers } from "@mutsuki/web-build";
import { SHARED_MODULES, SHARED_STYLESHEET } from "../src/modules.mjs";

const shared = path.join(path.dirname(fileURLToPath(import.meta.url)), "../browser/shared");
const specifiers = SHARED_MODULES.map((module) => module.specifier);

describe("browser/shared", () => {
  test("holds exactly the declared modules and stylesheet", async () => {
    const files = (await readdir(shared)).sort();
    expect(files).toEqual(
      [...SHARED_MODULES.map((module) => module.file), SHARED_STYLESHEET].sort(),
    );
  });

  test("modules import each other only through import-map specifiers", async () => {
    for (const module of SHARED_MODULES) {
      const source = await readFile(path.join(shared, module.file), "utf8");
      for (const specifier of await importSpecifiers(source)) {
        expect(specifiers, `${module.file} imports ${specifier}`).toContain(specifier);
        expect(specifier).not.toBe(module.specifier);
      }
    }
  });

  test("only vue.js carries the Vue runtime", async () => {
    for (const module of SHARED_MODULES) {
      const source = await readFile(path.join(shared, module.file), "utf8");
      expect(source.includes("__v_isVNode"), module.file).toBe(module.specifier === "vue");
    }
  });
});
