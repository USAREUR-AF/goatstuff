// SPDX-License-Identifier: Apache-2.0
//
// Vite config for the panoscope frontend.
//
// - Dev proxy: /api/** + /ws → backend on :8080 so `npm run dev` works
//   against a locally-running `cargo run` without CORS games.
// - Build output: dist/ — embedded into the Rust binary via rust-embed
//   in 82Q.6.

import { resolve } from "node:path";
import { defineConfig } from "vite";

export default defineConfig({
  root: ".",
  build: {
    outDir: "dist",
    emptyOutDir: true,
    sourcemap: true,
    rollupOptions: {
      // Multi-page: the COP plus the sibling top/who pages (served at
      // /top and /who by embed.rs).
      input: {
        main: resolve(__dirname, "index.html"),
        top: resolve(__dirname, "top.html"),
        who: resolve(__dirname, "who.html"),
        access: resolve(__dirname, "access.html"),
      },
    },
  },
  server: {
    port: 5173,
    strictPort: true,
    proxy: {
      "/api": {
        target: "http://127.0.0.1:8080",
        changeOrigin: false,
      },
      "/ws": {
        target: "ws://127.0.0.1:8080",
        ws: true,
        changeOrigin: false,
      },
    },
  },
});
