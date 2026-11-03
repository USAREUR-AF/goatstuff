//! CLI-through-fence authentication (ADR 1090). The web path is authenticated
//! by the oauth2-proxy sidecar, which sets `X-Forwarded-Groups` from a
//! validated Ory session — a browser redirect flow a terminal can't perform.
//! The CLI path instead presents the operator's portal key as a bearer:
//! microscope forwards it to the portal's `/api/whoami`, and a principal the
//! portal resolves as `operator: true` is admitted. A portal operator key is
//! the fleet's operator-tier credential (it is minted only by operator-tier
//! onboarding, alongside the identity-admin Ory groups the web gate checks),
//! so operator-tier is the CLI admission bar; the resolved handle becomes the
//! audited actor. Bearer requests reach the app on a Traefik route that
//! bypasses oauth2-proxy, so this path NEVER trusts `X-Forwarded-*` — the
//! bearer alone decides, which is what keeps the bypass route safe.

use serde::Deserialize;
use std::time::Duration;

#[derive(Deserialize)]
struct WhoamiResp {
    #[serde(default)]
    operator: bool,
    #[serde(default)]
    actor: String,
}

/// Validate an operator bearer against the portal. Returns the resolved
/// operator handle on success; `None` when the portal is unset/unreachable or
/// the principal the portal resolves is not an operator. Fail-closed by
/// construction — any error path is a `None` (refusal), never an admit.
pub async fn resolve_operator(portal_url: &str, bearer: &str) -> Option<String> {
    let portal_url = portal_url.trim_end_matches('/');
    if portal_url.is_empty() || bearer.is_empty() {
        return None;
    }
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .ok()?;
    let resp = http
        .get(format!("{portal_url}/api/whoami"))
        .bearer_auth(bearer)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let who: WhoamiResp = resp.json().await.ok()?;
    let actor = who.actor.trim();
    (who.operator && !actor.is_empty()).then(|| actor.to_string())
}

/// Extract a bearer token from the Authorization header, case-insensitively on
/// the scheme. `None` when absent or empty.
pub fn bearer_token(headers: &axum::http::HeaderMap) -> Option<String> {
    let v = headers.get("authorization")?.to_str().ok()?;
    let t = v
        .strip_prefix("Bearer ")
        .or_else(|| v.strip_prefix("bearer "))?
        .trim();
    (!t.is_empty()).then(|| t.to_string())
}
