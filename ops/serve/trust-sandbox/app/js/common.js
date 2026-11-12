// SPDX-License-Identifier: Apache-2.0
// Shared page helpers: the dark/light theme (restored before the page renders
// its dynamic parts) and a small element builder for lists rendered from data.

const saved = localStorage.getItem('tsbx-theme')
document.documentElement.setAttribute('data-theme', saved === 'light' ? 'light' : 'dark')

// Wire every .theme-toggle button on the page: it shows the theme it switches TO.
export function initTheme() {
  const buttons = document.querySelectorAll('.theme-toggle')
  const paint = () => {
    const dark = (document.documentElement.getAttribute('data-theme') || 'dark') === 'dark'
    for (const b of buttons) b.textContent = dark ? '◐ LIGHT' : '◑ DARK'
  }
  for (const b of buttons) {
    b.addEventListener('click', () => {
      const next = document.documentElement.getAttribute('data-theme') === 'light' ? 'dark' : 'light'
      document.documentElement.setAttribute('data-theme', next)
      localStorage.setItem('tsbx-theme', next)
      paint()
    })
  }
  paint()
}

// h('div', { class: 'x', onclick: fn }, 'text', childNode, [more]) → Element.
// Strings become text nodes (never parsed as HTML); null/false children are skipped.
export function h(tag, attrs, ...children) {
  const el = document.createElement(tag)
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v == null || v === false) continue
    if (k.startsWith('on') && typeof v === 'function') el.addEventListener(k.slice(2), v)
    else if (k === 'class') el.className = v
    else if (k === 'style' && typeof v === 'object') Object.assign(el.style, v)
    else if (v === true) el.setAttribute(k, '')
    else el.setAttribute(k, String(v))
  }
  const add = (c) => {
    if (c == null || c === false) return
    if (Array.isArray(c)) { c.forEach(add); return }
    el.appendChild(typeof c === 'string' || typeof c === 'number' ? document.createTextNode(String(c)) : c)
  }
  children.forEach(add)
  return el
}

// Set an element's text only when it changed (cheap to call every frame).
export function setText(el, text) {
  if (el && el.textContent !== text) el.textContent = text
}
