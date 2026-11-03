// SPDX-License-Identifier: Apache-2.0
//
// URL query-string → Target parsing. Three mutually-exclusive
// parameters: ?vendor= / ?topic= / ?system=. Anything else (or
// nothing) is a parse error rendered as a friendly first-paint
// page instead of an opaque 400.

use serde::Deserialize;
use thiserror::Error;

use stethoscope_checks::Target;

#[derive(Debug, Default, Deserialize)]
pub struct DiagnoseQuery {
    pub vendor: Option<String>,
    pub topic: Option<String>,
    pub system: Option<String>,
    /// Block 94 cross-surface deep-link contract: `?key=` is the
    /// canonical name for Zenoh key expressions (over stethoscope's
    /// legacy `?topic=`). Both accepted; `key=` wins on conflict.
    /// See docs/design/zenoh-surface-deep-links.md §"URL parameter
    /// canon".
    pub key: Option<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TargetParseError {
    #[error("no target parameter — pass exactly one of ?vendor= / ?topic= / ?system=")]
    Missing,
    #[error("more than one target parameter — pass exactly one of ?vendor= / ?topic= / ?system=")]
    Multiple,
    #[error("target value is empty")]
    Empty,
}

impl DiagnoseQuery {
    pub fn into_target(self) -> Result<Target, TargetParseError> {
        let mut count = 0;
        let mut t: Option<Target> = None;
        if let Some(v) = self.vendor.filter(|s| !s.is_empty()) {
            t = Some(Target::vendor(v));
            count += 1;
        }
        // `?key=` is the canonical alias for `?topic=` per the Block
        // 94 contract. When both are present, count them as one (key
        // wins) — they refer to the same Zenoh key expression, not
        // distinct targets.
        let topic_or_key = self
            .key
            .filter(|s| !s.is_empty())
            .or_else(|| self.topic.filter(|s| !s.is_empty()));
        if let Some(k) = topic_or_key {
            t = Some(Target::topic(k));
            count += 1;
        }
        if let Some(s) = self.system.filter(|s| !s.is_empty()) {
            t = Some(Target::system(s));
            count += 1;
        }
        match count {
            0 => Err(TargetParseError::Missing),
            1 => t.ok_or(TargetParseError::Empty),
            _ => Err(TargetParseError::Multiple),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stethoscope_checks::context::TargetKind;

    #[test]
    fn parses_vendor() {
        let q = DiagnoseQuery {
            vendor: Some("drs".into()),
            ..Default::default()
        };
        let t = q.into_target().unwrap();
        assert_eq!(t.kind, TargetKind::Vendor);
        assert_eq!(t.value, "drs");
    }

    #[test]
    fn parses_topic() {
        let q = DiagnoseQuery {
            topic: Some("drs/foo/bar/v1".into()),
            ..Default::default()
        };
        let t = q.into_target().unwrap();
        assert_eq!(t.kind, TargetKind::Topic);
        assert_eq!(t.value, "drs/foo/bar/v1");
    }

    #[test]
    fn parses_system() {
        let q = DiagnoseQuery {
            system: Some("drs/radar".into()),
            ..Default::default()
        };
        let t = q.into_target().unwrap();
        assert_eq!(t.kind, TargetKind::System);
    }

    #[test]
    fn rejects_no_params() {
        let q = DiagnoseQuery::default();
        assert_eq!(q.into_target().unwrap_err(), TargetParseError::Missing);
    }

    #[test]
    fn rejects_multiple_params() {
        let q = DiagnoseQuery {
            vendor: Some("drs".into()),
            topic: Some("drs/foo/bar/v1".into()),
            ..Default::default()
        };
        assert_eq!(q.into_target().unwrap_err(), TargetParseError::Multiple);
    }

    #[test]
    fn treats_empty_string_as_missing() {
        let q = DiagnoseQuery {
            vendor: Some(String::new()),
            ..Default::default()
        };
        assert_eq!(q.into_target().unwrap_err(), TargetParseError::Missing);
    }

    #[test]
    fn key_alias_works_like_topic() {
        // Block 94 cross-surface deep-link canon: `?key=` is the
        // preferred form, `?topic=` is the legacy alias.
        let q = DiagnoseQuery {
            key: Some("drs/foo/bar/v1".into()),
            ..Default::default()
        };
        let t = q.into_target().unwrap();
        assert_eq!(t.kind, TargetKind::Topic);
        assert_eq!(t.value, "drs/foo/bar/v1");
    }

    #[test]
    fn key_wins_over_topic_when_both_present() {
        // Same-meaning collision (a sibling surface might emit both
        // for backwards-compat). Treat as one target, not Multiple.
        let q = DiagnoseQuery {
            key: Some("vendor/key/v1".into()),
            topic: Some("vendor/topic/v1".into()),
            ..Default::default()
        };
        let t = q.into_target().unwrap();
        assert_eq!(t.kind, TargetKind::Topic);
        // key wins
        assert_eq!(t.value, "vendor/key/v1");
    }

    #[test]
    fn key_plus_vendor_still_rejected_as_multiple() {
        // Distinct kinds → genuine Multiple error.
        let q = DiagnoseQuery {
            key: Some("drs/foo/bar/v1".into()),
            vendor: Some("drs".into()),
            ..Default::default()
        };
        assert_eq!(q.into_target().unwrap_err(), TargetParseError::Multiple);
    }
}
