import { createApp } from 'vue'
import Vision from './Vision.vue'
import './style.css'

const saved = localStorage.getItem('tsbx-theme')
document.documentElement.setAttribute('data-theme', saved === 'light' ? 'light' : 'dark')

createApp(Vision).mount('#vision')
