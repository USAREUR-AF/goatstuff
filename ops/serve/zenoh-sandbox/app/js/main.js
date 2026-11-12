// SPDX-License-Identifier: Apache-2.0
// Entry point of the sandbox page (index.html): wire each panel to the page
// and re-render them all whenever the shared state in zenoh.js changes.
import { initTheme } from './dom.js'
import { z, onChange } from './zenoh.js'
import { initConnectBar } from './connect-bar.js'
import { initTopologyPanel } from './topology-panel.js'
import { initPublishPanel } from './publish-panel.js'
import { initSubscribePanel } from './subscribe-panel.js'
import { initTransformPanel } from './transform-panel.js'
import { initQueryPanel } from './query-panel.js'
import { initActivityLog } from './activity-log.js'

initTheme()

// Each init function binds its panel's controls and returns its render function.
const renders = [
  initConnectBar(),
  initTopologyPanel(),
  initPublishPanel(),
  initSubscribePanel(),
  initTransformPanel(),
  initQueryPanel(),
  initActivityLog(),
]

// Panels that need a live session are dimmed, and their buttons disabled,
// until the visitor connects.
const gatedPanels = document.querySelectorAll('.needs-link')
const gatedButtons = document.querySelectorAll('.needs-link button[id]')

function render() {
  for (const p of gatedPanels) p.classList.toggle('disabled-veil', !z.connected)
  for (const b of gatedButtons) b.disabled = !z.connected
  for (const r of renders) r()
}

onChange(render)
render()
