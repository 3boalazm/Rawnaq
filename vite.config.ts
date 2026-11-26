import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed port and its own console output left alone.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    // Audio samples, models and packages are large and written to while the
    // interface runs: watching them crashed the server (EBUSY on a .wav).
    watch: {
      ignored: ["**/src-tauri/**", "**/target/**", "**/crates/**", "**/samples/**", "**/models/**", "**/package/**"],
    },
  },
});
