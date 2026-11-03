//! The declared vocabulary, read from the files Ansible acts on.
//!
//! `netbird_acl_taxonomy.yml` carries a description for every group suffix and
//! `netbird_acl_matrix.yml` one for every rule. Both are compiled in with
//! `include_str!`, so what this service says a group is FOR cannot drift from
//! what `netbird-acl-bootstrap.yml` actually creates. A hand-written gloss
//! would have drifted the first time someone added a group.
//!
//! Keyed by SUFFIX, not by full group name. The taxonomy defines a shared
//! vocabulary and the prefix only says which scope an instance belongs to: a
//! goatnet carries site rungs below its own prefix (`netbird_exercises` stamps
//! the same suffix set under a site's prefix), and matching on full names against one
//! goatnet key once flagged an entire legitimate site as undeclared.

use std::collections::BTreeMap;

use panoscope_core::meshgraph::netvis::DeclaredSuffix;

pub const TAXONOMY_YAML: &str = include_str!("../../../ops/ansible/netbird_acl_taxonomy.yml");
pub const MATRIX_YAML: &str = include_str!("../../../ops/ansible/netbird_acl_matrix.yml");

/// The ACL declaration, parsed for the assertion evaluator.
///
/// The two files are compiled in HERE and not in `panoscope_core::aclcheck`,
/// which is a library: an `include_str!` at that level would force every image
/// linking it — microscope, stethoscope, panoscope, flashlight, none of which
/// read an ACL assertion — to ship `ops/ansible/*.yml` in order to compile,
/// and would rebuild all of them on a taxonomy edit. netvis already carries
/// these bytes for `group_purpose`; the assertion gate rides along.
pub fn declaration() -> Option<panoscope_core::aclcheck::Declaration> {
    panoscope_core::aclcheck::Declaration::parse(TAXONOMY_YAML, MATRIX_YAML).ok()
}

#[derive(serde::Deserialize)]
struct TaxonomyFile {
    netbird_acl_taxonomy: BTreeMap<String, Goatnet>,
    /// The gear classes an EXERCISE scope gets. Read as well as the per-site
    /// blocks: an exercise or site scope is instantiated from this
    /// template and its classes never appear under a goatnet key, so reading
    /// only the site block would leave every forward-scope machine unclassed.
    #[serde(default)]
    netbird_exercise_gear_template: Option<GearTemplate>,
    /// The scope rungs stamped BELOW a goatnet's own prefix, each naming the
    /// goatnet whose mgmt server hosts it. Read because "who may act on this
    /// machine" cannot be answered from the scope alone: a scope with no local
    /// admin of its own is the ordinary case, and the honest answer is the
    /// host's admins, not silence.
    #[serde(default)]
    netbird_exercises: BTreeMap<String, ExerciseScope>,
}
#[derive(serde::Deserialize)]
struct GearTemplate {
    #[serde(default)]
    groups: Vec<TaxGroup>,
}
#[derive(serde::Deserialize)]
struct ExerciseScope {
    site_prefix: String,
    /// A key of `netbird_acl_taxonomy` — resolved to that goatnet's own
    /// `site_prefix` rather than upper-cased here, because the two are not the
    /// same string (`acme-prod` → `ACME-PROD` happens to be, but that does
    /// not generalise) and guessing is how a rule acquires a
    /// second implementation.
    host_site: String,
}
#[derive(serde::Deserialize)]
struct Goatnet {
    site_prefix: String,
    groups: Vec<TaxGroup>,
}
#[derive(serde::Deserialize)]
struct TaxGroup {
    suffix: String,
    #[serde(default)]
    description: String,
    /// ADR 1116 §3 — the gear family this class belongs to, and which of the
    /// five slots it answers. Absent on non-gear groups (INFRA, DEVELOPER, …),
    /// which are roles rather than gear functions.
    #[serde(default)]
    family: Option<String>,
    #[serde(default)]
    slot: Option<String>,
}
#[derive(serde::Deserialize)]
struct MatrixFile {
    netbird_acl_matrix: BTreeMap<String, MatrixSite>,
}
#[derive(serde::Deserialize)]
struct MatrixSite {
    #[serde(default)]
    rules: Vec<MatrixRule>,
}
#[derive(serde::Deserialize)]
struct MatrixRule {
    name: String,
    #[serde(default)]
    description: String,
}

pub struct Doctrine {
    pub site_prefix: String,
    pub group_purpose: BTreeMap<String, String>,
    pub rule_purpose: BTreeMap<String, String>,
    /// suffix → (family, slot) for gear classes only (ADR 1116 §3).
    ///
    /// READ FROM THE DECLARATION, never rebuilt here. A table in this file
    /// would be a second implementation of the ontology, and the peer-name
    /// grammar already taught this tree what happens when a rule has more than
    /// one home. A taxonomy edit changes the binary — that is the point of the
    /// `include_str!` above.
    pub gear_ontology: BTreeMap<String, (String, String)>,
    /// scope prefix → the prefix of the goatnet that HOSTS that scope, e.g.
    /// `EX-DEMO` → `GALAXY`, `SITE-A` → `GALAXY-PROD`.
    ///
    /// Present so that an empty scope-local `PLATFORM-ADMIN` can be reported
    /// as what it is — a scope that has appointed no local admin — instead of
    /// as a gap. A goatnet's own prefix is absent from this map by
    /// construction: nothing hosts it, so an empty admin group there IS the
    /// thing worth saying out loud.
    pub scope_host_prefix: BTreeMap<String, String>,
}

impl Doctrine {
    pub fn declared_suffixes(&self) -> Vec<DeclaredSuffix> {
        self.group_purpose
            .iter()
            .map(|(suffix, purpose)| DeclaredSuffix {
                suffix: suffix.clone(),
                purpose: purpose.clone(),
            })
            .collect()
    }
}

/// Missing or unparseable is not fatal: the matrix still renders from live
/// data, just without the declared purpose. A doctrine read failure must never
/// blank the security picture.
pub fn load(site: &str) -> Option<Doctrine> {
    let site = site.trim().to_lowercase();
    let tax: TaxonomyFile = serde_yaml::from_str(TAXONOMY_YAML).ok()?;
    let mat: MatrixFile = serde_yaml::from_str(MATRIX_YAML).ok()?;
    let me = tax.netbird_acl_taxonomy.get(&site)?;
    Some(Doctrine {
        site_prefix: me.site_prefix.clone(),
        group_purpose: me
            .groups
            .iter()
            .map(|g| (g.suffix.clone(), g.description.clone()))
            .collect(),
        gear_ontology: me
            .groups
            .iter()
            .chain(
                tax.netbird_exercise_gear_template
                    .as_ref()
                    .map(|t| t.groups.iter())
                    .unwrap_or_else(|| [].iter()),
            )
            .filter_map(|g| {
                match (g.family.as_ref(), g.slot.as_ref()) {
                    (Some(f), Some(sl)) => Some((g.suffix.clone(), (f.clone(), sl.clone()))),
                    // Half-annotated is a declaration bug, not a class: a family
                    // with no slot cannot be placed in the five-slot pattern and
                    // a slot with no family cannot be grouped. Dropping it means
                    // the surface shows the class plainly rather than inventing
                    // a position for it.
                    _ => None,
                }
            })
            .collect(),
        scope_host_prefix: tax
            .netbird_exercises
            .values()
            .filter_map(|e| {
                // An unresolvable host_site drops the row rather than guessing
                // a prefix: naming the wrong goatnet's admins as answerable for
                // a machine is worse than saying nothing about where authority
                // sits.
                tax.netbird_acl_taxonomy
                    .get(&e.host_site)
                    .map(|h| (e.site_prefix.clone(), h.site_prefix.clone()))
            })
            .collect(),
        rule_purpose: mat
            .netbird_acl_matrix
            .get(&site)
            .map(|s| {
                s.rules
                    .iter()
                    .map(|r| (r.name.clone(), r.description.clone()))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The compiled-in taxonomy must parse for every live goatnet, and every
    /// declared group must say what it is for. This is what makes "write down
    /// what this group is for" a build requirement rather than a good
    /// intention.
    #[test]
    fn every_declared_group_states_its_purpose() {
        for site in ["zagros-prod", "zagros", "zagros-dev"] {
            let d = load(site).unwrap_or_else(|| panic!("no doctrine for {site}"));
            assert!(!d.site_prefix.is_empty(), "{site} has no prefix");
            assert!(!d.group_purpose.is_empty(), "{site} declares no groups");
            assert!(
                d.group_purpose.values().all(|p| !p.trim().is_empty()),
                "{site} has a declared group with no stated purpose"
            );
        }
    }

    /// Every site rung resolves to the goatnet that hosts it, and no goatnet
    /// resolves to one.
    ///
    /// The distinction is the whole point of the map. A scope WITH a host has
    /// somewhere to send a reader when its local PLATFORM-ADMIN is unappointed
    /// — the ordinary case, since the canonical group set is complete-by-prefix
    /// and most activities never appoint a local admin. A goatnet's own scope
    /// has nowhere to send them, so an empty PLATFORM-ADMIN there is the one
    /// version of this that is genuinely worth saying out loud.
    #[test]
    fn every_site_rung_names_its_host_and_no_goatnet_has_one() {
        let d = load("zagros").expect("zagros loads");
        // Every declared rung names a goatnet that exists. Read from the
        // taxonomy rather than listed here: which rungs exist is per-instance.
        let raw: TaxonomyFile = serde_yaml::from_str(TAXONOMY_YAML).expect("taxonomy parses");
        for (name, rung) in &raw.netbird_exercises {
            let host = raw
                .netbird_acl_taxonomy
                .get(&rung.host_site)
                .unwrap_or_else(|| panic!("{name}: host_site {} is not a goatnet", rung.host_site));
            assert_eq!(
                d.scope_host_prefix.get(&rung.site_prefix).map(String::as_str),
                Some(host.site_prefix.as_str()),
                "{name} must resolve to the prefix of the goatnet that hosts it"
            );
        }
        // Read from the same map on the other goatnet: the taxonomy is one
        // file, so the answer must not depend on which site loaded it.
        let p = load("zagros-prod").expect("zagros-prod loads");
        assert_eq!(p.scope_host_prefix, d.scope_host_prefix);

        for goatnet in ["ZAGROS", "ZAGROS-PROD", "ZAGROS-DEV"] {
            assert!(
                !d.scope_host_prefix.contains_key(goatnet),
                "{goatnet} is a goatnet, not a rung below one — nothing hosts it, and giving it a \
                 host would send a reader to someone with no authority over it"
            );
        }
    }
}

#[cfg(test)]
mod gear_ontology_tests {
    use super::*;

    /// The compiled-in taxonomy must classify every gear class, on every live
    /// goatnet, and classify NOTHING ELSE. A half-annotated declaration is the
    /// failure this catches: a family with no slot cannot be placed in the
    /// five-slot pattern, and a role group that accidentally acquired a family
    /// would be drawn as gear.
    #[test]
    fn every_gear_class_is_placed_and_no_role_group_is() {
        for site in ["zagros", "zagros-prod", "zagros-dev"] {
            let d = load(site).unwrap_or_else(|| panic!("{site} must load"));
            // The five slots of ADR 1116 §3 and nothing else.
            for (suffix, (family, slot)) in &d.gear_ontology {
                assert!(
                    matches!(
                        slot.as_str(),
                        "element" | "data-plane" | "telemetry" | "command-intake" | "arbiter"
                    ),
                    "{site}: {suffix} has slot {slot:?}, which is not one of the five"
                );
                assert!(!family.is_empty(), "{site}: {suffix} has an empty family");
            }
            // Roles are not gear. If INFRA or DEVELOPER ever acquires a family
            // the card would render a laptop as a sensor.
            for role in ["INFRA", "DEVELOPER", "PLATFORM-ADMIN", "OBSERVER", "MONITOR"] {
                assert!(
                    !d.gear_ontology.contains_key(role),
                    "{site}: {role} is a ROLE and must not carry a gear family"
                );
            }
        }
    }

    /// The sensor family decomposes into the slots the ADR names, and the two
    /// effector data-plane classes stay two classes.
    #[test]
    fn the_five_slot_pattern_is_what_the_adr_says() {
        let d = load("zagros-prod").expect("zagros-prod loads");
        let get = |s: &str| d.gear_ontology.get(s).cloned();
        assert_eq!(get("SENSOR-FEED"), Some(("SENSOR".into(), "data-plane".into())));
        assert_eq!(get("SENSOR-TASKING"), Some(("SENSOR".into(), "command-intake".into())));
        assert_eq!(get("SENSOR-TELEMETRY"), Some(("SENSOR".into(), "telemetry".into())));
        assert_eq!(get("SENSOR-ARBITER"), Some(("SENSOR".into(), "arbiter".into())));
        // TASKING and COMMAND are deliberately different words: pointing a
        // radar and ordering an engagement are not the same consequence, so
        // they must never collapse to one slot name across families.
        assert_eq!(get("EFFECTOR-COMMAND"), Some(("EFFECTOR".into(), "command-intake".into())));
        // CUE and TRACK SHARE a slot and remain TWO classes — identical on the
        // L4 plane, but QoS binds to identity (ADR 1033), so one class would
        // mean one identity and a fire-control track could not hold an
        // envelope a chatty cue stream cannot consume.
        assert_eq!(get("EFFECTOR-CUE"), Some(("EFFECTOR".into(), "data-plane".into())));
        assert_eq!(get("EFFECTOR-TRACK"), Some(("EFFECTOR".into(), "data-plane".into())));
        assert_ne!(get("EFFECTOR-CUE"), None);
    }

    /// SENSOR and EFFECTOR are RESERVED AND EMPTY (§3). They must still be
    /// CLASSIFIED — a reserved class is part of the pattern — but they are the
    /// `element` slot, which the renderer excludes from "does not" precisely
    /// so an empty-by-design class never reads as a gap.
    #[test]
    fn reserved_elements_are_classified_as_element() {
        let d = load("zagros-prod").expect("loads");
        assert_eq!(d.gear_ontology.get("SENSOR"), Some(&("SENSOR".into(), "element".into())));
        assert_eq!(d.gear_ontology.get("EFFECTOR"), Some(&("EFFECTOR".into(), "element".into())));
    }
}
