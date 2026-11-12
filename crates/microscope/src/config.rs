//! Env-driven config. Every source is optional — an unset source makes its
//! adapter report `skipped`, never a silent blank (design §Source adapters).

#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Listen address, e.g. `0.0.0.0:9750`.
    pub listen: String,
    /// Portal origin + operator bearer (registry, aliases, audit events, access).
    pub portal_url: String,
    pub portal_api_key: String,
    /// Kratos admin API origin (loopback on the mgmt host), e.g. `http://127.0.0.1:4434`.
    pub kratos_admin_url: String,
    /// Hydra ADMIN API origin. Kratos says a person authenticated; Hydra says
    /// WHICH SERVICE they were granted — the half of design source S6 that was
    /// never wired, which is why every timeline read as identical "Ory login"
    /// lines with no destination.
    pub hydra_admin_url: String,
    /// id-admin-console audit JSONL path (bind-mounted read-only in deploy).
    pub console_audit_path: String,
    /// NetBird management origin + admin PAT.
    pub netbird_url: String,
    pub netbird_token: String,
    /// MinIO audit-integrity tail (fabric-audit + operator-audit adapters).
    pub audit_endpoint: String,
    pub audit_region: String,
    pub audit_bucket: String,
    pub audit_key_prefix: String,
    pub audit_access_key: String,
    pub audit_secret_key: String,
    /// Lookback window for the audit adapters (hours).
    pub audit_window_hours: u32,
    /// Most timeline rows ONE audit adapter keeps from its window (the newest
    /// ones). 0 = [`crate::adapters::DEFAULT_AUDIT_MAX_ROWS`]. Aggregates
    /// (per-surface counts, totals) still cover every event in the window.
    pub audit_max_rows: usize,
    /// Site label shown in the page chrome (e.g. `zagros`).
    pub site_label: String,
    /// `off` (default, inert) | `enforce` (in-app fail-closed group gate).
    pub rbac_mode: String,
    /// Groups admitted when enforcing. Case-sensitive.
    pub rbac_who_groups: Vec<String>,
    /// Append-only JSONL of every look (who examined whom). Empty = off
    /// (dev only — the deploy role always sets it).
    pub view_audit_path: String,
    /// Block 94 sibling base URLs, exposed on /api/meta.
    pub siblings: std::collections::BTreeMap<String, String>,
    /// How long a typeahead candidate snapshot is served before rebuilding.
    /// The roster call behind it takes seconds on a large goatnet, so this
    /// is the difference between a search box and a portal load test.
    pub suggest_ttl_secs: u64,
}

impl Config {
    pub fn from_env() -> Self {
        let get = |k: &str| std::env::var(k).unwrap_or_default();
        Self {
            listen: {
                let v = get("MICROSCOPE_LISTEN");
                if v.is_empty() {
                    "127.0.0.1:9750".to_string()
                } else {
                    v
                }
            },
            portal_url: get("MICROSCOPE_PORTAL_URL"),
            portal_api_key: get("MICROSCOPE_PORTAL_API_KEY"),
            kratos_admin_url: get("MICROSCOPE_KRATOS_ADMIN_URL"),
            hydra_admin_url: get("MICROSCOPE_HYDRA_ADMIN_URL"),
            console_audit_path: get("MICROSCOPE_CONSOLE_AUDIT_PATH"),
            netbird_url: get("MICROSCOPE_NETBIRD_URL"),
            netbird_token: get("MICROSCOPE_NETBIRD_TOKEN"),
            audit_endpoint: get("MICROSCOPE_AUDIT_ENDPOINT"),
            audit_region: {
                let v = get("MICROSCOPE_AUDIT_REGION");
                if v.is_empty() {
                    "us-east-1".into()
                } else {
                    v
                }
            },
            audit_bucket: {
                let v = get("MICROSCOPE_AUDIT_BUCKET");
                if v.is_empty() {
                    "audit-integrity".into()
                } else {
                    v
                }
            },
            audit_key_prefix: get("MICROSCOPE_AUDIT_KEY_PREFIX"),
            audit_access_key: get("MICROSCOPE_AUDIT_ACCESS_KEY"),
            audit_secret_key: get("MICROSCOPE_AUDIT_SECRET_KEY"),
            audit_window_hours: parse_or(
                "MICROSCOPE_AUDIT_WINDOW_HOURS",
                &get("MICROSCOPE_AUDIT_WINDOW_HOURS"),
                24,
            ),
            audit_max_rows: parse_or(
                "MICROSCOPE_AUDIT_MAX_ROWS",
                &get("MICROSCOPE_AUDIT_MAX_ROWS"),
                0,
            ),
            site_label: {
                let v = get("MICROSCOPE_SITE_LABEL");
                if v.is_empty() {
                    "goatnet".into()
                } else {
                    v
                }
            },
            rbac_mode: {
                let v = get("MICROSCOPE_RBAC_MODE");
                if v.is_empty() {
                    "off".into()
                } else {
                    v
                }
            },
            rbac_who_groups: {
                let v = get("MICROSCOPE_RBAC_WHO_GROUPS");
                let v = if v.is_empty() {
                    // Matrix-vocabulary default (ADR 1073); per-goatnet
                    // group_vars may widen during rename windows.
                    "PLATFORM-ADMIN,IAM-ADMIN".to_string()
                } else {
                    v
                };
                v.split(',')
                    .map(|g| g.trim().to_string())
                    .filter(|g| !g.is_empty())
                    .collect()
            },
            view_audit_path: get("MICROSCOPE_VIEW_AUDIT_PATH"),
            suggest_ttl_secs: parse_or(
                "MICROSCOPE_SUGGEST_TTL_SECS",
                &get("MICROSCOPE_SUGGEST_TTL_SECS"),
                120,
            ),
            siblings: {
                // MICROSCOPE_SIBLING_<NAME>=<base-url> → {"<name>": url}
                let mut m = std::collections::BTreeMap::new();
                for (k, v) in std::env::vars() {
                    if let Some(name) = k.strip_prefix("MICROSCOPE_SIBLING_") {
                        if !v.is_empty() {
                            m.insert(name.to_lowercase(), v);
                        }
                    }
                }
                m
            },
        }
    }
}

fn parse_or<T: std::str::FromStr + std::fmt::Display>(name: &str, raw: &str, default: T) -> T {
    if raw.is_empty() {
        return default;
    }
    match raw.parse() {
        Ok(value) => value,
        Err(_) => {
            tracing::warn!(setting = name, value = ?raw, default = %default, "invalid numeric setting; using default");
            default
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_or;
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn capture<T>(operation: impl FnOnce() -> T) -> (T, String) {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let writer = Capture(Arc::clone(&bytes));
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .with_writer(move || writer.clone())
            .finish();
        let value = tracing::subscriber::with_default(subscriber, operation);
        let output = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
        (value, output)
    }

    #[test]
    fn valid_numeric_values_are_silent() {
        let ((window, rows, ttl), output) = capture(|| {
            (
                parse_or::<u32>("MICROSCOPE_AUDIT_WINDOW_HOURS", "48", 24),
                parse_or::<usize>("MICROSCOPE_AUDIT_MAX_ROWS", "0", 0),
                parse_or::<u64>("MICROSCOPE_SUGGEST_TTL_SECS", "180", 120),
            )
        });
        assert_eq!((window, rows, ttl), (48, 0, 180));
        assert!(output.is_empty(), "{output}");
    }

    #[test]
    fn unset_and_empty_keep_defaults_without_warning() {
        for raw in [None, Some("")] {
            let (value, output) = capture(|| {
                parse_or::<u32>("MICROSCOPE_AUDIT_WINDOW_HOURS", raw.unwrap_or_default(), 24)
            });
            assert_eq!(value, 24);
            assert!(output.is_empty(), "{output}");
        }
    }

    #[test]
    fn malformed_numeric_settings_warn_once_with_the_fallback() {
        for (name, raw, default) in [
            ("MICROSCOPE_AUDIT_WINDOW_HOURS", "24h", 24_u64),
            ("MICROSCOPE_AUDIT_MAX_ROWS", "-1", 0),
            ("MICROSCOPE_SUGGEST_TTL_SECS", "abc", 120),
        ] {
            let (value, output) = capture(|| parse_or(name, raw, default));
            assert_eq!(value, default);
            assert_eq!(output.lines().count(), 1, "{output}");
            assert!(output.contains("WARN"), "{output}");
            assert!(output.contains(name), "{output}");
            assert!(output.contains(raw), "{output}");
            assert!(output.contains(&format!("default={default}")), "{output}");
        }
    }

    #[test]
    fn overflow_warns_and_control_characters_cannot_forge_a_log_line() {
        for raw in ["4294967296", "24\nWARN injected"] {
            let (value, output) =
                capture(|| parse_or::<u32>("MICROSCOPE_AUDIT_WINDOW_HOURS", raw, 24));
            assert_eq!(value, 24);
            assert_eq!(output.lines().count(), 1, "{output}");
            if raw.contains('\n') {
                assert!(output.contains("24\\nWARN injected"), "{output}");
            }
        }
    }
}
