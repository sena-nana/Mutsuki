import vue from "@vitejs/plugin-vue";
import { defineConfig } from "vitest/config";

export default defineConfig({
  // `@mutsuki/ui` resolves to its SFC sources, so tests compile `.vue` like the build does.
  plugins: [vue()],
  // Resolve workspace packages to their TypeScript sources so tests need no prior build.
  resolve: { conditions: ["source"] },
  ssr: { resolve: { conditions: ["source"] } },
  test: {
    include: ["tests/**/*.test.{ts,mjs}"],
  },
});
