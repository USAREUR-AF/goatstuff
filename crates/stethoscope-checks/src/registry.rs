// SPDX-License-Identifier: Apache-2.0
//
// Registry — holds the enabled check set + dispatches a target to
// each check that applies. Streams `CheckOutcome` values as each
// `Check::run` future settles so the binary can SSE them to the
// browser in arrival order (the wireframe brief's "streaming-in
// feel: cards should not all appear at once").

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;

use futures::future::BoxFuture;
use futures::stream::FuturesUnordered;
use serde::Serialize;

use crate::check::Check;
use crate::context::{CheckCtx, Target};
use crate::outcome::CheckOutcome;

/// Per-check metadata the configure-checks drawer needs to render
/// without invoking `Check::run`. Cheap to serialize for the initial
/// page-render payload.
#[derive(Debug, Clone, Serialize)]
pub struct CheckDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: crate::check::CheckKind,
    pub cost: crate::check::Cost,
    pub description: &'static str,
}

#[derive(Clone)]
pub struct Registry {
    /// BTreeMap so list ordering is stable + deterministic for snapshot
    /// tests + reproducible UI ordering.
    checks: BTreeMap<&'static str, Arc<dyn Check>>,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            checks: BTreeMap::new(),
        }
    }

    /// Insert a check. Replaces a prior registration under the same id
    /// silently — keep `id()` unique across the binary build.
    pub fn register<C: Check + 'static>(&mut self, check: C) -> &mut Self {
        self.checks.insert(check.id(), Arc::new(check));
        self
    }

    pub fn len(&self) -> usize {
        self.checks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.checks.is_empty()
    }

    /// Read-only descriptor list — used by the initial page render to
    /// emit skeleton cards before any check has settled.
    pub fn descriptors(&self) -> Vec<CheckDescriptor> {
        self.checks
            .values()
            .map(|c| CheckDescriptor {
                id: c.id(),
                name: c.name(),
                kind: c.kind(),
                cost: c.cost(),
                description: c.description(),
            })
            .collect()
    }

    /// Subset of `descriptors()` that will actually produce a verdict
    /// against `target` (the rest land as `NotApplicable`).
    pub fn applicable_descriptors(&self, target: &Target) -> Vec<CheckDescriptor> {
        self.checks
            .values()
            .filter(|c| c.applies_to().contains(&target.kind))
            .map(|c| CheckDescriptor {
                id: c.id(),
                name: c.name(),
                kind: c.kind(),
                cost: c.cost(),
                description: c.description(),
            })
            .collect()
    }

    /// Resolve `target` against every registered check, yielding
    /// `CheckOutcome` values in arrival order. Checks that don't apply
    /// to the target's kind yield `Verdict::NotApplicable` immediately
    /// (cost zero — no `Check::run` call).
    ///
    /// Returns a `FuturesUnordered` stream of boxed futures driven on
    /// the calling task — no `tokio::spawn`. Per the repo-wide
    /// `clippy.toml` discipline (ADR 0709 D7 layer 3), task spawning
    /// is reserved for goat-relay's pinned-shard hot path; stethoscope
    /// is a request-shaped service and runs its check fan-out on the
    /// single axum handler task. The handler `.next().await`s the
    /// stream and forwards each settled outcome to the SSE channel.
    pub fn run<'a>(
        &'a self,
        ctx: CheckCtx,
        target: Target,
    ) -> FuturesUnordered<BoxFuture<'a, CheckOutcome>> {
        let pending: FuturesUnordered<BoxFuture<'a, CheckOutcome>> = FuturesUnordered::new();
        for check in self.checks.values() {
            let check: Arc<dyn Check> = Arc::clone(check);
            let ctx = ctx.clone();
            let target = target.clone();
            let fut: Pin<Box<dyn std::future::Future<Output = CheckOutcome> + Send + 'a>> =
                Box::pin(async move {
                    if !check.applies_to().contains(&target.kind) {
                        return CheckOutcome::not_applicable(check.id());
                    }
                    check.run(&ctx, &target).await
                });
            pending.push(fut);
        }
        pending
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::{Check, CheckKind, Cost};
    use crate::context::TargetKind;
    use crate::outcome::CheckOutcome;
    use async_trait::async_trait;
    use futures::stream::StreamExt;
    use panoscope_core::{AuditTail, AuditTailConfig, PortalClient};

    struct DummyVendorCheck;

    #[async_trait]
    impl Check for DummyVendorCheck {
        fn id(&self) -> &'static str {
            "dummy-vendor"
        }
        fn name(&self) -> &'static str {
            "Dummy"
        }
        fn kind(&self) -> CheckKind {
            CheckKind::Passive
        }
        fn cost(&self) -> Cost {
            Cost::None
        }
        fn applies_to(&self) -> &'static [TargetKind] {
            &[TargetKind::Vendor]
        }
        fn description(&self) -> &'static str {
            "dummy"
        }
        async fn run(&self, _ctx: &CheckCtx, _t: &Target) -> CheckOutcome {
            CheckOutcome::pass(self.id(), "ok")
        }
    }

    fn dummy_ctx() -> CheckCtx {
        let portal = PortalClient::new(String::new(), String::new());
        let audit = AuditTail::new(AuditTailConfig::default()).unwrap();
        CheckCtx::new(portal, audit)
    }

    #[tokio::test]
    async fn applicable_descriptors_filter_by_target_kind() {
        let mut reg = Registry::new();
        reg.register(DummyVendorCheck);
        let applicable = reg.applicable_descriptors(&Target::topic("foo/bar/v1"));
        assert!(
            applicable.is_empty(),
            "vendor-only check should not be applicable to a topic target"
        );
        let applicable = reg.applicable_descriptors(&Target::vendor("drs"));
        assert_eq!(applicable.len(), 1);
        assert_eq!(applicable[0].id, "dummy-vendor");
    }

    #[tokio::test]
    async fn run_emits_not_applicable_for_wrong_target_kind() {
        let mut reg = Registry::new();
        reg.register(DummyVendorCheck);
        let ctx = dummy_ctx();
        let mut stream = reg.run(ctx, Target::topic("foo/bar/v1"));
        let mut outcomes = Vec::new();
        while let Some(out) = stream.next().await {
            outcomes.push(out);
        }
        assert_eq!(outcomes.len(), 1);
        assert!(matches!(
            outcomes[0].verdict,
            crate::outcome::Verdict::NotApplicable
        ));
    }

    #[tokio::test]
    async fn run_invokes_check_for_matching_target_kind() {
        let mut reg = Registry::new();
        reg.register(DummyVendorCheck);
        let ctx = dummy_ctx();
        let mut stream = reg.run(ctx, Target::vendor("drs"));
        let mut outcomes = Vec::new();
        while let Some(out) = stream.next().await {
            outcomes.push(out);
        }
        assert_eq!(outcomes.len(), 1);
        assert!(matches!(outcomes[0].verdict, crate::outcome::Verdict::Pass));
    }

    #[test]
    fn registry_is_empty_by_default() {
        let reg = Registry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
    }
}
