// SPDX-License-Identifier: Apache-2.0
// Small DOM helpers shared by both pages: the dark/light theme, an element
// builder, and a keyed list renderer. Text always goes in as a text node, so
// nothing that arrives from the network is ever parsed as HTML.

const THEME_KEY = 'zsbx-theme'

// Restore the visitor's theme choice as soon as this module loads.
const saved = localStorage.getItem(THEME_KEY)
document.documentElement.setAttribute('data-theme', saved === 'light' ? 'light' : 'dark')

// Wire the .theme-toggle button: it names the theme it switches TO.
// onChange (optional) runs after each switch.
export function initTheme(onChange) {
  const buttons = document.querySelectorAll('.theme-toggle')
  const current = () => document.documentElement.getAttribute('data-theme') || 'dark'
  const paint = () => {
    const dark = current() === 'dark'
    for (const b of buttons) {
      b.textContent = dark ? '◐ LIGHT' : '◑ DARK'
      b.setAttribute('aria-label', 'switch to ' + (dark ? 'light' : 'dark') + ' mode')
    }
  }
  for (const b of buttons) {
    b.addEventListener('click', () => {
      const next = current() === 'dark' ? 'light' : 'dark'
      document.documentElement.setAttribute('data-theme', next)
      localStorage.setItem(THEME_KEY, next)
      paint()
      if (onChange) onChange(next)
    })
  }
  paint()
}

const SVG_NS = 'http://www.w3.org/2000/svg'

function build(el, attrs, children) {
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v == null || v === false) continue
    if (k.startsWith('on') && typeof v === 'function') el.addEventListener(k.slice(2), v)
    else if (k === 'class') el.setAttribute('class', v)
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

// h('div', { class: 'x', onclick: fn }, 'text', childNode, [more]) → Element.
// Strings become text nodes; null/false children are skipped.
export function h(tag, attrs, ...children) {
  return build(document.createElement(tag), attrs, children)
}

// The same builder for SVG elements.
export function s(tag, attrs, ...children) {
  return build(document.createElementNS(SVG_NS, tag), attrs, children)
}

// Set an element's text only when it changed.
export function setText(el, text) {
  if (el.textContent !== text) el.textContent = text
}

// Render `items` as the children of `parent`, reusing the element already
// showing each item (matched by keyOf(item)). An element that stays keeps its
// place and its running CSS animation; create(item) builds a new one;
// update(el, item) refreshes either kind.
export function syncList(parent, items, keyOf, create, update) {
  const have = new Map()
  for (const el of Array.from(parent.children)) have.set(el.__key, el)
  let prev = null
  for (const item of items) {
    const key = keyOf(item)
    let el = have.get(key)
    if (el) have.delete(key)
    else { el = create(item); el.__key = key }
    if (update) update(el, item)
    const want = prev ? prev.nextSibling : parent.firstChild
    if (el !== want) parent.insertBefore(el, want)
    prev = el
  }
  for (const el of have.values()) el.remove()
}

export function fmtTime(t) {
  return new Date(t).toLocaleTimeString([], { hour12: false })
}
