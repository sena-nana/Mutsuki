import { defineConfig } from "vite";

// The console entry ships as one module; the shell and everything below it come from the
// import map, so nothing shared is bundled here.
export default defineConfig({
  build: {
    outDir: "browser",
    emptyOutDir: true,
    target: "es2022",
    minify: true,
    lib: { entry: "src/main.ts", formats: ["es"], fileName: () => "console.js" },
    rolldownOptions: {
      external: ["vue", "vue-router", "pinia", "@mutsuki/web-sdk", "@mutsuki/ui", "@mutsuki/web-shell"],
      output: { minify: true },
    },
  },
});
