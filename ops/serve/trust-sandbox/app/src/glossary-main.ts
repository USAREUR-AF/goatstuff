import { createApp } from 'vue'
import Glossary from './Glossary.vue'
import './style.css'

const saved = localStorage.getItem('tsbx-theme')
document.documentElement.setAttribute('data-theme', saved === 'light' ? 'light' : 'dark')

createApp(Glossary).mount('#glossary')
