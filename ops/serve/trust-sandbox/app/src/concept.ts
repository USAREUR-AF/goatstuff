import { createApp } from 'vue'
import Concept from './Concept.vue'
import './style.css'

const saved = localStorage.getItem('tsbx-theme')
document.documentElement.setAttribute('data-theme', saved === 'light' ? 'light' : 'dark')

createApp(Concept).mount('#concept')
