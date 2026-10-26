import { resolve } from "node:path";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

/**
 * Builds the Goia surface bundle — godesk's canvas as a plugin Goia can host.
 *
 * Separate from the app's own build: a different root (`src/plugin`), its own
 * output (`dist-plugin/`), and `base: "./"` so every asset URL is RELATIVE. The
 * bundle is served from a loopback path `…/<extension id>/`, not a site root, so
 * an absolute `/assets/x.js` would resolve outside the extension's folder and
 * 404. `manifest.json` in `public/` is copied to the output root, which is the
 * drop-in folder a user installs in Goia.
 */
export default defineConfig({
  root: resolve(__dirname, "src/plugin"),
  base: "./",
  plugins: [react()],
  build: {
    outDir: resolve(__dirname, "dist-plugin"),
    emptyOutDir: true,
  },
});
