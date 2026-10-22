import { mkdir, mkdtemp, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, test } from "vitest";

import { buildExtension, checkBundle, importSpecifiers } from "../src/index.js";

const fixtures = path.join(path.dirname(fileURLToPath(import.meta.url)), "fixtures");
const temps: string[] = [];

async function tempDir(): Promise<string> {
  const dir = await mkdtemp(path.join(tmpdir(), "mutsuki-web-build-"));
  temps.push(dir);
  return dir;
}

afterEach(async () => {
  await Promise.all(temps.splice(0).map((dir) => rm(dir, { recursive: true, force: true })));
});

describe("buildExtension", () => {
  test("emits one ES module and one stylesheet with Vue left to the import map", async () => {
    const outDir = await tempDir();
    const result = await buildExtension({
      entry: path.join(fixtures, "ok/index.ts"),
      outDir,
      extensionId: "mutsuki.fixture",
      version: "1.2.3",
      capabilities: ["runtime.read"],
    });
    expect(result.styles).toEqual(["index.css"]);
    expect(result.assets.map((asset) => asset.path).sort()).toEqual(["index.css", "index.js"]);
    const source = await readFile(result.entryFile, "utf8");
    expect(await importSpecifiers(source)).toEqual(["vue"]);
    const manifest = JSON.parse(await readFile(result.manifestPath!, "utf8"));
    expect(manifest).toMatchObject({
      manifest_version: 1,
      id: "mutsuki.fixture",
      entry: "index.js",
      styles: ["index.css"],
      capabilities: ["runtime.read"],
    });
    expect(manifest.assets[0].content_hash).toMatch(/^sha256:[0-9a-f]{64}$/);
  });

  test("leaves the manifest to an embedding crate when asked", async () => {
    const outDir = await tempDir();
    const result = await buildExtension({
      entry: path.join(fixtures, "ok/index.ts"),
      outDir,
      extensionId: "mutsuki.fixture",
      version: "1.2.3",
      writeManifest: false,
    });
    expect(result.manifestPath).toBeNull();
    expect((await readdir(outDir)).sort()).toEqual(["index.css", "index.js"]);
  });

  test("refuses node modules and raw colors", async () => {
    await expect(
      buildExtension({
        entry: path.join(fixtures, "bad/index.ts"),
        outDir: await tempDir(),
        extensionId: "mutsuki.bad",
        version: "0.0.0",
      }),
    ).rejects.toThrow(/node_import[\s\S]*css_hex_color|css_hex_color[\s\S]*node_import/);
  });
});

describe("checkBundle", () => {
  test("flags bare imports, inlined Vue runtime and budgets", async () => {
    const outDir = await tempDir();
    await mkdir(path.join(outDir, "nested"));
    await writeFile(
      path.join(outDir, "index.js"),
      'import { h } from "vue"; import x from "lodash"; const v = { __v_isVNode: true }; export { h, x, v };',
    );
    await writeFile(path.join(outDir, "nested/big.css"), `.a{color:var(--x)}${" ".repeat(64)}`);
    const violations = await checkBundle(outDir, { budgets: { cssBytes: 16 } });
    expect(violations.map((item) => [item.file, item.rule])).toEqual([
      ["index.js", "bare_import"],
      ["index.js", "vue_runtime_bundled"],
      ["nested/big.css", "css_budget"],
    ]);
  });

  test("id selectors are not colors", async () => {
    const outDir = await tempDir();
    await writeFile(path.join(outDir, "index.css"), "#abc .x { color: var(--mk-color-text); }");
    expect(await checkBundle(outDir)).toEqual([]);
  });
});
