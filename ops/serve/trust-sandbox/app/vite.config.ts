import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

// Two Vue-rendered pages share this build + image: the playground (index.html)
// and the stakeholder vision page (vision.html). The OIDC login/consent/callback
// pages are plain static HTML in public/ (served verbatim by nginx). The app is
// same-origin: it talks to the orchestrator at /api and /ws (see nginx.conf).
export default defineConfig({
  plugins: [vue()],
  base: '/',
  build: {
    target: 'esnext',
    rollupOptions: {
      input: { main: 'index.html', vision: 'vision.html', concept: 'concept.html', glossary: 'glossary.html', deep: 'deep.html' },
    },
  },
})
