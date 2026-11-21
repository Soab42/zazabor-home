import { defineConfig } from "vite";

const host = process.env.TAURI_DEV_HOST;

/** The Android TV app (`mobile-tv/`): only the TV section, from `mobile-tv.html`, into `dist-mobile-tv/`. */
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1440,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1441 } : undefined,
    watch: { ignored: ["**/src-tauri/**", "**/mobile/**", "**/mobile-tv/**"] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    target: "es2022",
    outDir: "dist-mobile-tv",
    emptyOutDir: true,
    rollupOptions: { input: "mobile-tv.html" },
    minify: !process.env.TAURI_ENV_DEBUG,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
});
