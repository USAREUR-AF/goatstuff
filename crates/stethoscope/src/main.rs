// SPDX-License-Identifier: Apache-2.0
//
// stethoscope binary entry. Reads STETHOSCOPE_* env vars, builds the
// PortalClient + AuditTail handles, registers Phase B checks, and
// binds an axum server. See docs/design/stethoscope.md.

use std::sync::Arc;

use anyhow::{Context, Result};
use panoscope_core::AuditTail;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use stethoscope_checks::{checks, CheckCtx, Registry};

mod config;
mod render;
mod routes;
mod sse;
mod target;

use config::Config;
use routes::{router, AppState};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,hyper=warn,reqwest=warn")))
        .init();

    let cfg = Config::from_env().context("load env config")?;
    info!(bind = %cfg.bind, site = %cfg.site_label, portal_enabled = !cfg.portal_base_url.is_empty(), audit_enabled = cfg.audit.enabled, "stethoscope starting");
    if cfg.portal_base_url.is_empty() {
        warn!("STETHOSCOPE_PORTAL_URL is empty — all portal-dependent checks will report Skipped");
    }
    if !cfg.audit.enabled {
        warn!("STETHOSCOPE_AUDIT_ENABLED=false — flow-rate check will report Skipped");
    }

    let portal = cfg.portal_client();
    let audit = AuditTail::new(cfg.audit.clone()).context("build audit tail")?;
    let ctx = CheckCtx::new(portal, audit).with_org(cfg.lexicon.org.clone());

    let mut registry = Registry::new();
    checks::register_phase_b(&mut registry);
    info!(checks = registry.len(), "registered Phase B checks");

    let state = AppState {
        registry: Arc::new(registry),
        ctx,
        site_label: cfg.site_label.clone(),
        index_cache: Arc::new(tokio::sync::Mutex::new(Default::default())),
        siblings: cfg.siblings.clone(),
        lexicon: cfg.lexicon.clone(),
    };

    let app = router(state);
    let listener = tokio::net::TcpListener::bind(cfg.bind)
        .await
        .with_context(|| format!("bind {}", cfg.bind))?;
    info!(listening = %cfg.bind, "stethoscope ready");
    axum::serve(listener, app).await.context("axum serve")?;

    Ok(())
}
