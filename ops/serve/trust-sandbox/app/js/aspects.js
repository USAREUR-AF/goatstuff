// SPDX-License-Identifier: Apache-2.0
// The canonical symbology for the five aspects of identity & access. We use a
// distinct LETTER (not a number — numbers imply a sequence these don't have),
// each color-coded and mnemonic to its word: N-etwork, M-achine, W-orkload,
// H-uman, D-ecision. Rendered the SAME way everywhere — a bold chip in text and
// diagrams, and spelled out (leading cap emphasized) where there's room — so it
// stays legible and learnable, including for readers scanning quickly.

export const ASPECT_LIST = [
  { id: 'netbird', sym: 'N', word: 'Network', color: '#5aa9e6', who: 'machines · entry', title: 'Network admission' },
  { id: 'cert', sym: 'M', word: 'Machine', color: '#34d6bb', who: 'machines', title: 'Machine identity (x509)' },
  { id: 'spire', sym: 'W', word: 'Workload', color: '#b98cff', who: 'processes', title: 'Workload identity (SPIRE)' },
  { id: 'ory', sym: 'H', word: 'Human', color: '#f2b441', who: 'humans', title: 'Human identity + role (Ory)' },
  { id: 'acl', sym: 'D', word: 'Decision', color: '#46d39a', who: 'per action', title: 'The access decision' },
]
export const ASPECT_BY_ID = Object.fromEntries(ASPECT_LIST.map((a) => [a.id, a]))
export const SYM_ORDER = ['N', 'M', 'W', 'H', 'D']
export const SYM_COLOR = { N: '#5aa9e6', M: '#34d6bb', W: '#b98cff', H: '#f2b441', D: '#46d39a' }
