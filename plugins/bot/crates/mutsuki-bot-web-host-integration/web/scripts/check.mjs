import path from "node:path";
import { fileURLToPath } from "node:url";
import { SHARED_EXTERNALS, checkBundle, formatViolations } from "@mutsuki/web-build";

const outDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "../browser");
const violations = await checkBundle(outDir, {
  allowedImports: [...SHARED_EXTERNALS, "@mutsuki/web-shell"],
});
if (violations.length) {
  console.error(formatViolations(violations));
  process.exit(1);
}
