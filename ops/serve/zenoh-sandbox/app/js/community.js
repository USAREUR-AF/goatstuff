// SPDX-License-Identifier: Apache-2.0
// Entry point of the "community of opportunity" page (community.html): the
// theme toggle, the TODAY / ON THE FABRIC control, and the canvas scene.
import { initTheme } from './dom.js'
import { CommunityViz } from './community-viz.js'

const viz = new CommunityViz(document.getElementById('community-canvas'))
initTheme(() => viz.refreshTheme())

const todayBtn = document.getElementById('mode-today')
const fabricBtn = document.getElementById('mode-fabric')
const perMode = document.querySelectorAll('[data-mode]') // the caption and legend of each view

let shown = ''
function paint(mode) {
  if (mode === shown) return
  shown = mode
  todayBtn.className = mode === 'today' ? 'on today' : ''
  fabricBtn.className = mode === 'fabric' ? 'on' : ''
  for (const el of perMode) el.hidden = el.dataset.mode !== mode
}

todayBtn.addEventListener('click', () => { viz.setMode('today'); paint('today') })
fabricBtn.addEventListener('click', () => { viz.setMode('fabric'); paint('fabric') })

viz.start()
paint(viz.mode)
// The scene moves from TODAY to the fabric by itself after a few seconds;
// keep the control and the caption in step with it.
setInterval(() => paint(viz.mode), 150)
