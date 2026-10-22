import { defineConfig } from "vitest/config";

export default defineConfig({
  // Resolve workspace packages to their TypeScript sources so tests need no prior build.
  resolve: { conditions: ["source"] },
  ssr: { resolve: { conditions: ["source"] } },
  test: {
    include: ["tests/**/*.test.{ts,mjs}"],
  },
});
