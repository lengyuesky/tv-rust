import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      "/api": "http://127.0.0.1:55320",
      "/media": "http://127.0.0.1:55320",
    },
  },
  build: {
    rollupOptions: {
      output: {
        manualChunks: { player: ["hls.js"], react: ["react", "react-dom"] },
      },
    },
  },
});
