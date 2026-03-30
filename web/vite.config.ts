import react from "@vitejs/plugin-react";
import { resolve } from "path";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: { "@": resolve(__dirname, "src") },
  },
  server: {
    port: 5173,
    fs: { allow: [".."] }, // allow ?raw imports from ocaml/golden/
    proxy: {
      "/api": {
        target: "http://localhost:3001",
        rewrite: (path) => path.replace(/^\/api/, ""),
      },
    },
  },
  // Allow ?raw imports from outside src/
  assetsInclude: ["**/*.md"],
  optimizeDeps: {
    include: [
      "use-sync-external-store/shim/with-selector",
      "use-sync-external-store/shim",
    ],
  },
});
