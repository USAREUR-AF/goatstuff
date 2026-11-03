// SPDX-License-Identifier: Apache-2.0
//
// build.rs — ensures `frontend/dist/` exists at compile time so
// rust-embed has bytes to inline into the binary.
//
// Behavior:
//   1. If `frontend/dist/index.html` exists → no-op (use what's there).
//   2. Else if `npm` is on PATH → run `npm install` then `npm run build`
//      in `frontend/`.
//   3. Else → emit cargo:warning + create a minimal placeholder so
//      compilation can proceed (e.g. on build machines without Node);
//      operator gets a clear "run `npm run build` in frontend/" hint
//      via the placeholder index.html.
//
// Re-runs on changes to frontend sources so editing TS or CSS rebuilds
// the bundle on the next `cargo build`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let frontend = crate_dir.join("frontend");
    let dist = frontend.join("dist");
    let index_html = dist.join("index.html");

    // Re-run on frontend source changes so cargo picks up edits.
    println!("cargo:rerun-if-changed=frontend/package.json");
    println!("cargo:rerun-if-changed=frontend/index.html");
    println!("cargo:rerun-if-changed=frontend/vite.config.ts");
    println!("cargo:rerun-if-changed=frontend/tsconfig.json");
    for entry in walk_dir(&frontend.join("src")) {
        println!("cargo:rerun-if-changed={}", entry.display());
    }

    if index_html.exists() && is_dist_fresh(&frontend, &index_html) {
        return;
    }

    if !command_available("npm") {
        println!(
            "cargo:warning=panoscope: frontend/dist not present and `npm` not on PATH. \
             Run `npm install && npm run build` in frontend/ before building the Rust crate. \
             Inserting a placeholder so the build can proceed."
        );
        ensure_placeholder(&dist);
        return;
    }

    println!("cargo:warning=panoscope: building frontend (npm run build)…");
    run_npm(&frontend, &["install"]);
    run_npm(&frontend, &["run", "build"]);

    if !index_html.exists() {
        panic!(
            "panoscope: frontend build did not produce {}; check `npm run build` output",
            index_html.display()
        );
    }
}

fn is_dist_fresh(frontend: &Path, index_html: &Path) -> bool {
    // Compare mtime of any frontend/src/** file against the built
    // index.html. If any source is newer, treat dist as stale.
    let dist_mtime = match index_html.metadata().and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let src_dir = frontend.join("src");
    if !src_dir.exists() {
        return true;
    }
    for entry in walk_dir(&src_dir) {
        if let Ok(meta) = entry.metadata() {
            if let Ok(src_mtime) = meta.modified() {
                if src_mtime > dist_mtime {
                    return false;
                }
            }
        }
    }
    true
}

fn walk_dir(p: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(p) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk_dir(&path));
        } else {
            out.push(path);
        }
    }
    out
}

fn command_available(cmd: &str) -> bool {
    Command::new(cmd)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run_npm(cwd: &Path, args: &[&str]) {
    let status = Command::new("npm")
        .args(args)
        .current_dir(cwd)
        .status()
        .unwrap_or_else(|e| panic!("panoscope: failed to spawn `npm {}`: {e}", args.join(" ")));
    if !status.success() {
        panic!(
            "panoscope: `npm {}` exited with status {}",
            args.join(" "),
            status
        );
    }
}

fn ensure_placeholder(dist: &Path) {
    std::fs::create_dir_all(dist).expect("create dist dir");
    let placeholder = dist.join("index.html");
    if !placeholder.exists() {
        std::fs::write(
            &placeholder,
            r#"<!doctype html>
<html><head><title>panoscope — frontend not built</title></head>
<body style="font-family: sans-serif; padding: 2em;">
  <h1>panoscope frontend not built</h1>
  <p>This binary was compiled without a built frontend bundle.</p>
  <p>To fix: <code>cd ops/panoscope/frontend &amp;&amp; npm install &amp;&amp; npm run build</code>
     then rebuild the Rust crate.</p>
</body></html>
"#,
        )
        .expect("write placeholder index.html");
    }
}
