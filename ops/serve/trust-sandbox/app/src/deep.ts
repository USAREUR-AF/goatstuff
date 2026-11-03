import { createApp } from 'vue'
import Deep from './Deep.vue'
import './style.css'

const saved = localStorage.getItem('tsbx-theme')
document.documentElement.setAttribute('data-theme', saved === 'light' ? 'light' : 'dark')

createApp(Deep).mount('#deep')
