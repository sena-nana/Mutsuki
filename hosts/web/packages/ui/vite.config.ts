import vue from "@vitejs/plugin-vue";
import { defineConfig } from "vite";

// Library build: one ES module (Vue and the SDK stay import-map specifiers) plus the
// component stylesheet that bundle-css.mjs appends to dist/mutsuki-ui.css.
export default defineConfig({
  plugins: [vue()],
  resolve: { conditions: ["source"] },
  define: { "process.env.NODE_ENV": JSON.stringify("production") },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "es2022",
    minify: false,
    cssCodeSplit: false,
    lib: {
      entry: "src/index.ts",
      formats: ["es"],
      fileName: () => "index.js",
      cssFileName: "components",
    },
    rolldownOptions: {
      external: ["vue", "@mutsuki/web-sdk"],
    },
  },
});
