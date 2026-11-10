// SPDX-License-Identifier: Apache-2.0
// vision.html — the stakeholder "bigger picture" page. The copy is static HTML;
// this mounts the canvas illustrations into their [data-viz] slots.
import { initTheme } from './common.js'
import { initGlossPopup } from './gloss-popup.js'
import { mountTrustViz } from './trust-viz.js'
import { mountBlastRadius } from './blast-radius.js'
import { mountEncryptionViz } from './encryption-viz.js'

initTheme()
for (const el of document.querySelectorAll('[data-viz="trust"]')) mountTrustViz(el, el.dataset.world)
for (const el of document.querySelectorAll('[data-viz="blast"]')) mountBlastRadius(el, el.dataset.world)
for (const el of document.querySelectorAll('[data-viz="enc"]')) mountEncryptionViz(el, el.dataset.mode)
initGlossPopup()
