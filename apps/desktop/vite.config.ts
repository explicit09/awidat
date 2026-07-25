import { defineConfig, type Plugin } from "vite";
import { resolve } from "node:path";
import { rm } from "node:fs/promises";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// Keep the stage-harness fixtures (video clip + images + scene JSONs,
// ~1.5MB) out of the production bundle. They live under `public/` so the
// dev server serves them at `/fixtures/*` for the harness, but nothing in
// the app loads them at runtime — only the dev-only `/stage-harness` route
// does. `apply: "build"` scopes this to `tauri build`; dev serving is
// untouched.
function dropHarnessFixtures(): Plugin {
  return {
    name: "drop-harness-fixtures",
    apply: "build",
    async closeBundle() {
      const outDir = resolve(__dirname, "dist/fixtures");
      await rm(outDir, { recursive: true, force: true });
    },
  };
}

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react(), tailwindcss(), dropHarnessFixtures()],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
  build: {
    rollupOptions: {
      input: {
        main: resolve(__dirname, "index.html"),
        gallery: resolve(__dirname, "gallery.html"),
        shell: resolve(__dirname, "shell.html"),
        glass: resolve(__dirname, "glass.html"),
        studio: resolve(__dirname, "studio.html"),
      },
    },
  },
}));
