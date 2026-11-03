// SPDX-License-Identifier: Apache-2.0
//
// The panoscope view model: one server-owned answer to "what does this viewer
// see". Design of record: docs/design/panoscope-view-model.md.
//
//   subs   — incremental topic → subscribers index
//   model  — the fabric model, rebuilt on a tick, shared by every viewer
//   group  — the single grouping rule (families + per-publisher budget)

pub mod assemble;
pub mod cache;
pub mod group;
pub mod model;
pub mod service;
pub mod subs;

/// FNV-1a, 64-bit. Deterministic across processes and Rust versions, which
/// `std`'s `DefaultHasher` does not promise; group ids and generation
/// fingerprints hash with it.
pub struct Fnv(u64);

impl Fnv {
    pub fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    pub fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= *b as u64;
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
    pub fn write_u64(&mut self, v: u64) {
        self.write(&v.to_le_bytes());
    }
    pub fn finish(&self) -> u64 {
        self.0
    }
}
