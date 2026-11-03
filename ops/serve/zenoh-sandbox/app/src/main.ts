import { createApp } from 'vue'
import App from './App.vue'
import './style.css'

// Restore the visitor's theme choice before mount to avoid a flash.
const saved = localStorage.getItem('zsbx-theme')
document.documentElement.setAttribute('data-theme', saved === 'light' ? 'light' : 'dark')

createApp(App).mount('#app')
