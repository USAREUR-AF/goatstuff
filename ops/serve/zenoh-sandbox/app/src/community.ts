import { createApp } from 'vue'
import Community from './Community.vue'
import './style.css'

// Restore the visitor's theme choice before mount to avoid a flash (shared
// with the sandbox via the same localStorage key).
const saved = localStorage.getItem('zsbx-theme')
document.documentElement.setAttribute('data-theme', saved === 'light' ? 'light' : 'dark')

createApp(Community).mount('#app')
