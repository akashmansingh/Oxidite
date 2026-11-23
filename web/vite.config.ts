import { defineConfig } from "vite";
// Gateway address is a placeholder: match whatever `oxidited` binds to.
const gateway = "127.0.0.1:8787";
export default defineConfig({
  server: { proxy: { "/ws": { target: `ws://${gateway}`, ws: true }, "/api": `http://${gateway}` } },
});
