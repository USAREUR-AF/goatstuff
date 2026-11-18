//! Conservative inference from a rendered router policy, never live attestation.
use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use zenoh::key_expr::KeyExpr;

#[derive(Clone, Debug, Default, Serialize)]
pub struct Attribution {
    pub namespace_slot: Option<String>,
    pub namespace_attribution: String,
    pub policy_digest: Option<String>,
    pub policy_evidence: String,
}

#[derive(Clone, Default)]
pub struct Policy {
    slots: BTreeSet<String>,
    digest: Option<String>,
    writes: Vec<(KeyExpr<'static>, String)>,
}

fn strings(value: &Value) -> Result<Vec<&str>> {
    value
        .as_array()
        .context("policy array missing")?
        .iter()
        .map(|v| v.as_str().context("policy string missing"))
        .collect()
}

impl Policy {
    pub fn load(path: Option<&std::path::Path>) -> Result<Self> {
        path.map(|p| {
            Self::parse(&std::fs::read_to_string(p).context("read rendered router policy")?)
        })
        .transpose()
        .map(Option::unwrap_or_default)
    }

    pub fn parse(raw: &str) -> Result<Self> {
        let config: Value = json5::from_str(raw).context("parse rendered router policy")?;
        let mut result = Self {
            slots: BTreeSet::new(),
            writes: Vec::new(),
            digest: Some(format!(
                "sha256:{}",
                hex::encode(Sha256::digest(raw.as_bytes()))
            )),
        };
        let acl = &config["access_control"];
        // Decline inference on unsecured or forwarding configurations. A local
        // artifact does not prove which configuration a running router loaded.
        if acl["enabled"] != true
            || acl["default_permission"] != "deny"
            || config.pointer("/transport/link/tls/enable_mtls") != Some(&Value::Bool(true))
            || config
                .pointer("/connect/endpoints")
                .is_some_and(|v| !v.as_array().is_some_and(Vec::is_empty))
            || config.pointer("/scouting/multicast/enabled") != Some(&Value::Bool(false))
            || config.pointer("/scouting/gossip/enabled") != Some(&Value::Bool(false))
        {
            return Ok(result);
        }
        let Ok(listeners) = strings(&config["listen"]["endpoints"]) else {
            return Ok(result);
        };
        if listeners.is_empty()
            || listeners
                .iter()
                .any(|s| !(s.starts_with("tls/") || s.starts_with("quic/")))
        {
            return Ok(result);
        }
        let mut subjects = HashMap::new();
        for subject in acl["subjects"].as_array().context("subjects missing")? {
            let id = subject["id"].as_str().context("subject id missing")?;
            anyhow::ensure!(subjects.insert(id, subject).is_none(), "duplicate subject");
        }
        let mut rules = HashMap::new();
        for rule in acl["rules"].as_array().context("rules missing")? {
            let id = rule["id"].as_str().context("rule id missing")?;
            anyhow::ensure!(rules.insert(id, rule).is_none(), "duplicate rule");
        }
        let mut grants = Vec::new();
        for policy in acl["policies"].as_array().context("policies missing")? {
            let ids = strings(&policy["subjects"])?;
            for rule_id in strings(&policy["rules"])? {
                let rule = rules.get(rule_id).context("unknown rule reference")?;
                anyhow::ensure!(
                    rule["permission"] == "allow" || rule["permission"] == "deny",
                    "unknown permission"
                );
                if rule["permission"] == "deny" {
                    continue;
                } // ignoring denies is conservative
                if !strings(&rule["flows"])?.contains(&"ingress") {
                    continue;
                }
                let messages = strings(&rule["messages"])?;
                if !messages.iter().any(|m| *m == "put" || *m == "delete") {
                    continue;
                }
                for id in &ids {
                    let subject = subjects.get(id).context("unknown subject reference")?;
                    let cn = subject["cert_common_names"]
                        .as_array()
                        .filter(|v| v.len() == 1)
                        .and_then(|v| v[0].as_str())
                        .filter(|_| {
                            subject.as_object().is_some_and(|o| {
                                o.keys().all(|k| k == "id" || k == "cert_common_names")
                            })
                        });
                    for key in strings(&rule["key_exprs"])? {
                        grants.push((
                            KeyExpr::try_from(key.to_string())
                                .map_err(|e| anyhow::anyhow!("invalid policy key: {e}"))?,
                            cn,
                        ));
                    }
                }
            }
        }
        for (id, subject) in subjects {
            let Some(slot) = id.strip_prefix("slot-") else {
                continue;
            };
            if slot.len() != 32
                || !slot
                    .bytes()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            {
                continue;
            }
            if subject["cert_common_names"] != serde_json::json!([slot]) {
                continue;
            }
            let namespace = KeyExpr::try_from(format!("{slot}/**"))
                .map_err(|e| anyhow::anyhow!("slot namespace: {e}"))?;
            let matches: Vec<_> = grants
                .iter()
                .filter(|(key, _)| key.intersects(&namespace))
                .collect();
            if !matches.is_empty() && matches.iter().all(|(_, cn)| *cn == Some(slot)) {
                result.slots.insert(slot.to_string());
                result.writes.extend(
                    matches
                        .into_iter()
                        .map(|(key, _)| (key.clone(), slot.to_owned())),
                );
            }
        }
        Ok(result)
    }

    pub fn attribute(&self, key: &str) -> Attribution {
        let concrete = KeyExpr::try_from(key.to_owned()).ok();
        let slot = key.split('/').next().filter(|s| {
            self.slots.contains(*s)
                && concrete.as_ref().is_some_and(|k| {
                    self.writes
                        .iter()
                        .any(|(pattern, owner)| owner == s && pattern.includes(k))
                })
        });
        Attribution {
            namespace_slot: slot.map(str::to_owned),
            namespace_attribution: if slot.is_some() {
                "namespace_acl_inferred"
            } else {
                "unknown"
            }
            .into(),
            policy_digest: self.digest.clone(),
            policy_evidence: if self.digest.is_some() {
                "rendered_configuration"
            } else {
                "unavailable"
            }
            .into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inference_requires_exclusive_secure_policy() {
        let slot = "a".repeat(32);
        let mut v = serde_json::json!({"listen":{"endpoints":["tls/localhost:7447"]},"transport":{"link":{"tls":{"enable_mtls":true}}},"access_control":{"enabled":true,"default_permission":"deny","subjects":[{"id":format!("slot-{slot}"),"cert_common_names":[slot]}],"rules":[{"id":"write","flows":["ingress"],"messages":["put","delete"],"permission":"allow","key_exprs":[format!("{slot}/**")]}],"policies":[{"rules":["write"],"subjects":[format!("slot-{slot}")]}]}});
        v["scouting"] = serde_json::json!({"multicast":{"enabled":false},"gossip":{"enabled":false}});
        let key = format!("{slot}/data");
        for pointer in ["/scouting/multicast/enabled", "/scouting/gossip/enabled"] {
            for setting in [Value::Null, Value::Bool(true)] {
                let mut uncertain = v.clone();
                *uncertain.pointer_mut(pointer).unwrap() = setting;
                assert!(Policy::parse(&uncertain.to_string()).unwrap().slots.is_empty());
            }
        }
        for endpoints in [Value::Null, serde_json::json!({"router":[]}), serde_json::json!(["tls/remote:7447"])] {
            let mut uncertain = v.clone(); uncertain["connect"] = serde_json::json!({"endpoints":endpoints});
            assert!(Policy::parse(&uncertain.to_string()).unwrap().slots.is_empty());
        }
        for endpoints in [Value::Null, serde_json::json!([]), serde_json::json!(["tcp/localhost:7447"])] {
            let mut uncertain = v.clone(); uncertain["listen"]["endpoints"] = endpoints;
            assert!(Policy::parse(&uncertain.to_string()).unwrap().slots.is_empty());
        }
        let mut narrow = v.clone(); narrow["access_control"]["rules"][0]["key_exprs"] = serde_json::json!([format!("{slot}/allowed/**")]);
        assert!(Policy::parse(&narrow.to_string()).unwrap().attribute(&key).namespace_slot.is_none());
        let p = Policy::parse(&v.to_string()).unwrap();
        assert_eq!(
            p.attribute(&key).namespace_slot.as_deref(),
            Some(slot.as_str())
        );
        assert_eq!(p.attribute(&key).policy_evidence, "rendered_configuration");
        assert_eq!(
            p.attribute("multipub/shared/x").namespace_attribution,
            "unknown"
        );
        v["access_control"]["subjects"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"id":"other","cert_common_names":["other"]}));
        v["access_control"]["policies"][0]["subjects"]
            .as_array_mut()
            .unwrap()
            .push(Value::String("other".into()));
        assert_eq!(
            Policy::parse(&v.to_string())
                .unwrap()
                .attribute(&key)
                .namespace_attribution,
            "unknown"
        );
        v["connect"] = serde_json::json!({"endpoints":["tls/remote:7447"]});
        assert!(Policy::parse(&v.to_string()).unwrap().slots.is_empty());
        assert_eq!(
            Policy::default().attribute(&key).policy_evidence,
            "unavailable"
        );
    }
}
