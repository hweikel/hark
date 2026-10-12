import { defineConfig } from "vite";

export default defineConfig({
  // Tauri serves the frontend from this dev server and expects a fixed port.
  server: {
    port: 1420,
    strictPort: true,
  },
  // Tauri handles its own minification/sourcemaps story; keep the output plain.
  build: {
    target: "es2021",
    minify: false,
    sourcemap: true,
  },
  clearScreen: false,
});
