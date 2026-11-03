// SPDX-License-Identifier: Apache-2.0
//
// Serves the Tier 2 WASM artifacts (`flashlight_analytics_bg.wasm` +
// `flashlight_analytics.js` glue) produced by the Dockerfile's
// `wasm-builder` stage. Phase E foundation per
// docs/design/flashlight.md — the Tier 1 page lazy-loads these from
// `/api/wasm/<file>` when conditions allow; a follow-on PR wires the
// boot logic on the frontend side.
//
// Embedding strategy: `rust-embed` walks `src/assets/wasm/` at
// compile time. The folder is populated by the Dockerfile's
// wasm-builder stage; for local non-Docker builds the folder is
// empty (only `.gitkeep` lives there) and the route 404s. Phase E
// doesn't require the WASM artifacts to exist during local dev —
// the Tier 1 page degrades gracefully when the load fails.

use axum::{
    extract::Path,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "src/assets/wasm/"]
struct WasmAssets;

/// Long cache for content-hashed asset filenames; very short cache
/// otherwise. wasm-bindgen by default uses fixed filenames
/// (`flashlight_analytics_bg.wasm`, `flashlight_analytics.js`), so
/// the bytes change without a URL change — keep cache short here
/// and let the frontend force a fresh fetch when needed.
const SHORT_CACHE: &str = "public, max-age=60, must-revalidate";

/// `GET /api/wasm/{filename}` — serves a single file out of the
/// embedded `src/assets/wasm/` tree. Returns 404 with an empty body
/// when the asset isn't embedded (local non-Docker build, or a
/// typo'd filename). Content-Type is mapped from the file extension:
/// `application/wasm` for `.wasm`, `application/javascript` for
/// `.js`, falls through to mime-guess otherwise.
pub async fn wasm_asset_handler(Path(filename): Path<String>) -> Response {
    // Reject anything with a path separator — defense in depth even
    // though rust-embed's lookup is by exact key.
    if filename.contains('/') || filename.contains("..") {
        return (StatusCode::BAD_REQUEST, "invalid filename").into_response();
    }

    let Some(file) = WasmAssets::get(&filename) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };

    let content_type = if filename.ends_with(".wasm") {
        "application/wasm"
    } else if filename.ends_with(".js") {
        "application/javascript"
    } else {
        // mime-guess for anything else (sourcemaps, etc.).
        return (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, file.metadata.mimetype().to_string()),
                (header::CACHE_CONTROL, SHORT_CACHE.to_string()),
            ],
            file.data.to_vec(),
        )
            .into_response();
    };

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type.to_string()),
            (header::CACHE_CONTROL, SHORT_CACHE.to_string()),
        ],
        file.data.to_vec(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::Path as AxumPath;

    #[tokio::test]
    async fn rejects_path_traversal() {
        let resp = wasm_asset_handler(AxumPath("../etc/passwd".to_string())).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn rejects_slash() {
        let resp = wasm_asset_handler(AxumPath("sub/file.wasm".to_string())).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn unknown_filename_404s() {
        // Local build has no artifacts embedded → every request 404s.
        let resp = wasm_asset_handler(AxumPath("not_a_real_file.wasm".to_string())).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
