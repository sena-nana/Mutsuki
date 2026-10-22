import vue from "@vitejs/plugin-vue";
import { defineConfig } from "vite";

// Library build of the shell: one ES module; vue, vue-router, pinia, the SDK and the UI stay
// import-map specifiers. Shell chrome styles live in @mutsuki/ui (console.css), so no CSS here.
export default defineConfig({
  plugins: [vue()],
  resolve: { conditions: ["source"] },
  define: { "process.env.NODE_ENV": JSON.stringify("production") },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "es2022",
    minify: false,
    lib: { entry: "src/index.ts", formats: ["es"], fileName: () => "index.js" },
    rolldownOptions: {
      external: ["vue", "vue-router", "pinia", "@mutsuki/web-sdk", "@mutsuki/ui"],
    },
  },
});
