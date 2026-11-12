// SPDX-License-Identifier: Apache-2.0
// "The fabric link" panel: the server URL, Connect / Disconnect, and the
// connection status.
import { setText } from './dom.js'
import { z } from './zenoh.js'

const LABELS = { idle: 'not connected', connecting: 'connecting…', connected: 'on the fabric', error: 'connection trouble' }

export function initConnectBar() {
  const url = document.getElementById('conn-url')
  const connect = document.getElementById('conn-connect')
  const disconnect = document.getElementById('conn-disconnect')
  const status = document.getElementById('conn-status')
  const label = document.getElementById('conn-status-label')
  const error = document.getElementById('conn-error')

  url.value = z.url
  url.addEventListener('input', () => { z.url = url.value })
  connect.addEventListener('click', () => z.connect())
  disconnect.addEventListener('click', () => z.disconnect())

  return function render() {
    const connecting = z.status === 'connecting'
    url.disabled = z.connected || connecting
    connect.hidden = z.connected
    connect.disabled = connecting
    setText(connect, connecting ? 'Linking…' : 'Connect')
    disconnect.hidden = !z.connected
    status.className = 'status ' + z.status
    setText(label, LABELS[z.status])
    error.hidden = z.status !== 'error'
    setText(error, z.error)
  }
}
