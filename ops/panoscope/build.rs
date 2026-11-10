// SPDX-License-Identifier: Apache-2.0
//
// build.rs — tells cargo to rebuild when anything under `frontend/web/`
// changes. rust-embed (src/embed.rs) inlines that directory into the
// binary; the pages are plain HTML, CSS and JavaScript, so there is no
// frontend build step. Watching the directory also catches added and
// removed files, which per-file include tracking would miss.

fn main() {
    println!("cargo:rerun-if-changed=frontend/web");
}
