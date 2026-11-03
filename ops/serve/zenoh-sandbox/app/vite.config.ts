import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import wasm from 'vite-plugin-wasm'
import topLevelAwait from 'vite-plugin-top-level-await'

// zenoh-ts ships a keyexpr WASM module + uses top-level await, so those two
// plugins are required; esnext target lets esbuild emit the TLA/destructuring
// the WASM glue needs (see docs/design/zenoh-sandbox-ux.md "Stack").
//
// Two pages share this build + image: the sandbox (index.html) and the
// "community of opportunity" vision page (community.html) — separate pages,
// cross-linked, served from the same nginx root at sandbox.nova.
export default defineConfig({
  plugins: [vue(), wasm(), topLevelAwait()],
  base: '/',
  build: {
    target: 'esnext',
    chunkSizeWarningLimit: 1500,
    rollupOptions: {
      input: { main: 'index.html', community: 'community.html' },
    },
  },
})
