// SPDX-License-Identifier: Apache-2.0
//
// rust-embed bundle for the frontend SPA. Built into the Rust binary
// at compile time via build.rs → `npm run build` → `frontend/dist/`.
//
// The axum fallback handler serves the SPA: requests to `/` return
// `index.html`; requests to `/assets/<file>` return the bundled
// asset with the right Content-Type. All other paths fall through to
// `index.html` so client-side routing (if v2 adds it) works.

use axum::body::Body;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

use crate::config::Lexicon;

#[derive(Embed)]
#[folder = "frontend/dist/"]
struct Frontend;

/// Serves the embedded SPA bundle, applying display-lexicon token
/// substitution to HTML responses (`__LEX_*__` → resolved wording).
/// Non-HTML assets pass through byte-identical. Mirror of the portal's
/// serve-time HTML substitution (`docs/design/portal-lexicon.md`):
/// the default lexicon resolves the tokens to the "vendor" / "<site>
/// fabric" wording (byte-stable), an org lexicon to "org" / "<site> Fabric".
pub async fn serve_embedded(uri: Uri, lexicon: &Lexicon) -> Response {
    let raw = uri.path().trim_start_matches('/');
    // Empty path → index.html; the sibling top/who pages get clean paths.
    let candidate = match raw {
        "" => "index.html",
        "top" => "top.html",
        "who" => "who.html",
        // /access + the R.3 identity deep-link route (ADR 1026:
        // `<cop>/access/identity?id=<identity_id>`) both render the
        // access page; the JS reads the path + query.
        "access" | "access/identity" => "access.html",
        other => other,
    };

    if let Some(file) = Frontend::get(candidate) {
        return file_response(candidate, file, lexicon);
    }
    // SPA fallback — unknown paths render index.html for client-side
    // routing. Returns 404 only if even index.html is missing (which
    // would mean the build placeholder is in use).
    if let Some(index) = Frontend::get("index.html") {
        return file_response("index.html", index, lexicon);
    }
    (StatusCode::NOT_FOUND, "panoscope: no frontend bundle").into_response()
}

fn file_response(path: &str, file: rust_embed::EmbeddedFile, lexicon: &Lexicon) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    // Only HTML carries lexicon tokens. Apply substitution there; every
    // other asset (JS bundle, fonts) passes through untouched. The JS
    // bundle reads its runtime wording from /api/legend instead.
    let body = if mime.essence_str() == "text/html" {
        Body::from(substitute(&file.data, lexicon))
    } else {
        Body::from(file.data.into_owned())
    };
    Response::builder()
        .header(header::CONTENT_TYPE, mime.as_ref())
        .body(body)
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Single non-overlapping pass replacing each `__LEX_*__` token with its
/// resolved value. A resolved value that happens to contain a token
/// string is never re-substituted because we scan the source once.
fn substitute(raw: &[u8], lexicon: &Lexicon) -> Vec<u8> {
    // HTML is UTF-8; if a non-UTF-8 byte ever slips in, serve verbatim
    // rather than corrupting it.
    let Ok(text) = std::str::from_utf8(raw) else {
        return raw.to_vec();
    };
    if !text.contains("__LEX_") {
        return raw.to_vec();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    'outer: while let Some(idx) = rest.find("__LEX_") {
        out.push_str(&rest[..idx]);
        let tail = &rest[idx..];
        for (token, value) in lexicon.tokens() {
            if let Some(after) = tail.strip_prefix(token) {
                out.push_str(value);
                rest = after;
                continue 'outer;
            }
        }
        // Unknown __LEX_… sequence — emit the marker verbatim and advance
        // past it so we don't loop forever.
        let (marker, after) = tail.split_at("__LEX_".len());
        out.push_str(marker);
        rest = after;
    }
    out.push_str(rest);
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_html_is_embedded() {
        let f = Frontend::get("index.html");
        assert!(
            f.is_some(),
            "frontend/dist/index.html should be embedded after `npm run build`"
        );
    }

    #[tokio::test]
    async fn root_path_returns_index_html() {
        let resp = serve_embedded("/".parse().unwrap(), &Lexicon::default()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp.headers().get(header::CONTENT_TYPE).unwrap();
        assert!(
            ct.to_str().unwrap().starts_with("text/html"),
            "expected text/html, got {ct:?}"
        );
    }

    #[tokio::test]
    async fn unknown_path_falls_back_to_index_html() {
        let resp =
            serve_embedded("/nonexistent/path".parse().unwrap(), &Lexicon::default()).await;
        // SPA fallback — non-API paths get index.html (client-side router).
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[test]
    fn substitute_resolves_tokens_single_pass() {
        let zagros = Lexicon {
            org: "org".into(),
            orgs: "orgs".into(),
            org_upper: "ORG".into(),
            orgs_upper: "ORGS".into(),
            fabric_label: "ZAGROS Fabric".into(),
        };
        let src = b"panoscope \xE2\x80\x94 __LEX_FABRIC__ / __LEX_ORGS__ / __LEX_ORG_UPPER__ SUPER";
        let out = String::from_utf8(substitute(src, &zagros)).unwrap();
        assert_eq!(out, "panoscope — ZAGROS Fabric / orgs / ORG SUPER");
    }

    #[test]
    fn substitute_jb_default_is_byte_stable() {
        // The default lexicon must render the historical wording verbatim.
        let src = b"<title>panoscope \xE2\x80\x94 __LEX_FABRIC__</title> __LEX_ORGS__";
        let out = String::from_utf8(substitute(src, &Lexicon::default())).unwrap();
        assert_eq!(out, "<title>panoscope — Zenoh fabric</title> vendors");
    }

    #[test]
    fn substitute_passthrough_when_no_tokens() {
        let src = b"no tokens here";
        assert_eq!(substitute(src, &Lexicon::default()), src.to_vec());
    }

    // End-to-end against the actual embedded dist: an org lexicon must
    // resolve every token in the served index.html — no `__LEX_` leaks,
    // "org" wording present, "Zenoh fabric" gone. Guards against a
    // future index.html edit that introduces an unmatched token.
    #[tokio::test]
    async fn served_index_html_substitutes_zagros_lexicon() {
        let zagros = Lexicon {
            org: "org".into(),
            orgs: "orgs".into(),
            org_upper: "ORG".into(),
            orgs_upper: "ORGS".into(),
            fabric_label: "ZAGROS Fabric".into(),
        };
        let resp = serve_embedded("/".parse().unwrap(), &zagros).await;
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(bytes.to_vec()).unwrap();
        // A real build is embedded (skip the assertion on the stub
        // placeholder used when `npm run build` hasn't run).
        if html.contains("PANOSCOPE") {
            assert!(
                !html.contains("__LEX_"),
                "unresolved lexicon token in served index.html"
            );
            assert!(html.contains("panoscope — ZAGROS Fabric"));
            assert!(!html.contains("Zenoh fabric"));
        }
    }
}
