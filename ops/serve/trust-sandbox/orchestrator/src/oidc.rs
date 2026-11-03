//! The Ory/Hydra OIDC path (gate ④) — authenticate the actor + carry the role.
//!
//! Hydra is the real OAuth2/OIDC provider; the orchestrator is its login/consent
//! app (a demo-persona picker — see the design doc honesty matrix: the JWT/role
//! is LIVE, the Kratos credential store is represented by the picker in v1).
//! `groups` lands in the ID token at consent; minting is gated on it carrying
//! ADMIN — the delegated-authority story (an ADMIN may issue for others).

use anyhow::{anyhow, Context, Result};
use jsonwebtoken::{decode, decode_header, jwk::JwkSet, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Clone, Deserialize)]
pub struct Claims {
    pub sub: String,
    #[serde(default)]
    pub groups: Vec<String>,
    pub exp: usize,
}

pub struct Oidc {
    http: reqwest::Client,
    admin: String,
    jwks_url: String,
    issuer: String,
    client_id: String,
}

impl Oidc {
    pub fn new(admin: &str, public_internal: &str, issuer: &str, client_id: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            admin: admin.trim_end_matches('/').to_string(),
            jwks_url: format!("{}/.well-known/jwks.json", public_internal.trim_end_matches('/')),
            issuer: issuer.trim_end_matches('/').to_string(),
            client_id: client_id.to_string(),
        }
    }

    /// Accept a Hydra login for `subject` (the chosen persona). Returns redirect_to.
    pub async fn accept_login(&self, challenge: &str, subject: &str) -> Result<String> {
        let url = format!(
            "{}/admin/oauth2/auth/requests/login/accept?login_challenge={}",
            self.admin, challenge
        );
        let r = self
            .http
            .put(&url)
            .json(&json!({ "subject": subject, "remember": false }))
            .send()
            .await
            .context("hydra login accept")?;
        redirect_of(r).await
    }

    /// Read the subject Hydra recorded for this consent challenge.
    pub async fn consent_subject(&self, challenge: &str) -> Result<String> {
        let url = format!(
            "{}/admin/oauth2/auth/requests/consent?consent_challenge={}",
            self.admin, challenge
        );
        let v: serde_json::Value = self
            .http
            .get(&url)
            .send()
            .await
            .context("hydra get consent")?
            .json()
            .await?;
        v.get("subject")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("consent request had no subject"))
    }

    /// Accept consent, injecting the role into the ID token's `groups` claim.
    pub async fn accept_consent(&self, challenge: &str, role: &str) -> Result<String> {
        let url = format!(
            "{}/admin/oauth2/auth/requests/consent/accept?consent_challenge={}",
            self.admin, challenge
        );
        let body = json!({
            "grant_scope": ["openid", "offline", "groups"],
            "grant_access_token_audience": [],
            "remember": false,
            "session": { "id_token": { "groups": [role] } }
        });
        let r = self
            .http
            .put(&url)
            .json(&body)
            .send()
            .await
            .context("hydra consent accept")?;
        redirect_of(r).await
    }

    /// Validate an ID token against Hydra's JWKS, issuer, and audience.
    pub async fn verify(&self, token: &str) -> Result<Claims> {
        let header = decode_header(token).context("decode jwt header")?;
        let kid = header.kid.ok_or_else(|| anyhow!("jwt has no kid"))?;
        let jwks: JwkSet = self
            .http
            .get(&self.jwks_url)
            .send()
            .await
            .context("fetch jwks")?
            .json()
            .await?;
        let jwk = jwks
            .find(&kid)
            .ok_or_else(|| anyhow!("no jwk for kid {kid}"))?;
        let key = DecodingKey::from_jwk(jwk).context("jwk → key")?;
        let mut v = Validation::new(Algorithm::RS256);
        v.set_issuer(&[&self.issuer]);
        v.set_audience(&[&self.client_id]);
        let data = decode::<Claims>(token, &key, &v).context("jwt verify")?;
        Ok(data.claims)
    }
}

async fn redirect_of(r: reqwest::Response) -> Result<String> {
    let status = r.status();
    let v: serde_json::Value = r.json().await.context("hydra json")?;
    v.get("redirect_to")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("hydra {status}: no redirect_to in {v}"))
}
