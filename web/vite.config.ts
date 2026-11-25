import { defineConfig } from "vite";

const env = (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env ?? {};
const port = env.OXIDITE_PORT || "3030";
const gateway = env.OXIDITE_GATEWAY || `127.0.0.1:${port}`;

export default defineConfig({
  server: {
    proxy: {
      "/ws": { target: `ws://${gateway}`, ws: true, changeOrigin: true },
      "/api": { target: `http://${gateway}`, changeOrigin: true },
    },
  },
});
