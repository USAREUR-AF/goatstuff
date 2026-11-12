// SPDX-License-Identifier: Apache-2.0
//! Wire types for `ops/fabric/topology/<site>/<node>`.
//!
//! prost-derive on hand-written structs, so there is no `protoc` in the build
//! and no `build.rs`. `proto/router_topology_v1.proto` is the canonical
//! human-facing schema — **keep the field tags here in lock-step with it**.

use prost::Message;
use serde::Serialize;

/// How this router's `gateway.south` is configured, read from the DECLARED
/// config file. Never inferred from observed regions: `Auto` and a `Custom`
/// list matching nothing are observationally identical, and the second is a
/// misconfiguration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ::prost::Enumeration, Serialize)]
#[repr(i32)]
pub enum GatewayMode {
    Unspecified = 0,
    Auto = 1,
    Custom = 2,
    Unreadable = 3,
}

impl GatewayMode {
    pub fn label(self) -> &'static str {
        match self {
            GatewayMode::Unspecified => "unspecified",
            GatewayMode::Auto => "auto",
            GatewayMode::Custom => "custom",
            GatewayMode::Unreadable => "unreadable",
        }
    }
}

/// Which axes one `gateway.south` filter uses. Values are deliberately NOT
/// carried — the surface needs to know an axis is in use (finding T-001), not
/// what it matches, and shipping ZID allow-lists onto the bus would disclose
/// more than the surface needs.
#[derive(Clone, PartialEq, ::prost::Message, Serialize)]
pub struct GatewayFilterV1 {
    #[prost(bool, tag = "1")]
    pub uses_modes: bool,
    #[prost(bool, tag = "2")]
    pub uses_interfaces: bool,
    /// Peer-asserted.
    #[prost(bool, tag = "3")]
    pub uses_zids: bool,
    /// Peer-asserted.
    #[prost(bool, tag = "4")]
    pub uses_region_names: bool,
    #[prost(bool, tag = "5")]
    pub negated: bool,
}

impl GatewayFilterV1 {
    /// True when this filter leans on an axis the peer asserts about itself.
    pub fn uses_peer_asserted_axis(&self) -> bool {
        self.uses_zids || self.uses_region_names
    }
}

/// One entry of the `gateway.south` Custom list. `index` IS the region id.
#[derive(Clone, PartialEq, ::prost::Message, Serialize)]
pub struct GatewaySubregionV1 {
    #[prost(uint32, tag = "1")]
    pub index: u32,
    /// `false` = the `filters` key was absent, which matches EVERY peer.
    /// `true` with an empty `filters` vec = an empty list, which matches NONE.
    #[prost(bool, tag = "2")]
    pub filters_present: bool,
    #[prost(message, repeated, tag = "3")]
    pub filters: Vec<GatewayFilterV1>,
}

impl GatewaySubregionV1 {
    /// An explicitly empty filter list matches nothing, so the subregion is
    /// inert and every peer silently falls north. Finding T-002.
    pub fn is_inert(&self) -> bool {
        self.filters_present && self.filters.is_empty()
    }
}

#[derive(Clone, PartialEq, ::prost::Message, Serialize)]
pub struct SessionV1 {
    #[prost(string, tag = "1")]
    pub peer_zid: String,
    #[prost(string, tag = "2")]
    pub whatami: String,
    #[prost(string, tag = "3")]
    pub region: String,
    #[prost(string, repeated, tag = "4")]
    pub link_dst: Vec<String>,
}

/// Whether a `Custom` gateway admits a default-configured client.
///
/// A router whose `gateway.south` is a Custom list puts every peer that matches
/// no subregion in NORTH. A client is never allowed north of a router, so the
/// client refuses the session (`Remote's custom configuration conflicts with
/// auto preset`, zenoh-1.9.0 net/runtime/region.rs:58). A Custom list with no
/// subregion that matches clients therefore refuses EVERY default client —
/// applications, meters, and this tool's own publisher. Finding T-013.
///
/// `Unspecified` is what a publisher older than this field sends, and what
/// `Auto` and `Unreadable` carry: no claim is made, so no finding fires.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ::prost::Enumeration, Serialize)]
#[repr(i32)]
pub enum ClientAdmission {
    Unspecified = 0,
    /// At least one subregion matches clients (or might: an `interfaces` or a
    /// negated filter is counted as admitting, so this never cries wolf).
    Admitted = 1,
    /// No subregion can match a client. Every default client is refused.
    Refused = 2,
}

/// One router-to-router link in a region's link-state graph, by ZID.
///
/// Carried so the surface can draw routers that send no snapshot of their own
/// (a partner's router, a host without a publisher): a router two hops away is
/// known only from these pairs, never from this router's sessions.
#[derive(Clone, PartialEq, ::prost::Message, Serialize)]
pub struct LinkV1 {
    #[prost(string, tag = "1")]
    pub a: String,
    #[prost(string, tag = "2")]
    pub b: String,
}

/// The most links one region carries on the wire. A region this large is far
/// past the size the T-009 curve says to split at; the cap keeps a snapshot
/// bounded whatever the fabric does, and `links_truncated` says it was hit.
pub const MAX_LINKS_PER_REGION: usize = 1024;

#[derive(Clone, PartialEq, ::prost::Message, Serialize)]
pub struct RegionV1 {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(uint32, tag = "2")]
    pub link_state_v: u32,
    #[prost(uint32, tag = "3")]
    pub link_state_e: u32,
    #[prost(string, repeated, tag = "4")]
    pub member_zids: Vec<String>,
    #[prost(uint32, tag = "5")]
    pub session_count: u32,
    /// Every edge of this region's link-state graph, as ZID pairs. Empty from a
    /// publisher older than this field; the surface then falls back to drawing
    /// only this router's own sessions.
    #[prost(message, repeated, tag = "6")]
    pub links: Vec<LinkV1>,
    #[prost(bool, tag = "7")]
    pub links_truncated: bool,
}

impl RegionV1 {
    /// This region's contribution to the all-pairs next-hop table.
    ///
    /// `V(V−1)` — **but only for a region that has edges**. A region with `E=0`
    /// contributes nothing however many nodes it lists: with no links there are
    /// no paths to compute successors for.
    ///
    /// The edge condition is not cosmetic: a `south:0:peer` region can carry
    /// **V=3, E=0** — nodes known through gossip that this router holds no link to.
    /// Naive `Σ V(V−1)` predicts 8 there against an actual table of 2, so the T-005
    /// finding would fire a false warning on a healthy router on every cycle. With
    /// the edge condition the model fits every shape below:
    ///
    /// | regions                           | predicted | observed |
    /// |-----------------------------------|-----------|----------|
    /// | north V2 E1; south:0:peer V3 E0   | 2         | 2        |
    /// | north V3 E2; south:0:peer V1 E0   | 6         | 6        |
    /// | north V1 E0; south:0:peer V1 E0   | 0         | 0        |
    /// | north V3 E2; south:0:router V3 E2 | 12        | 12       |
    pub fn successor_terms(&self) -> u32 {
        if self.link_state_e == 0 {
            return 0;
        }
        self.link_state_v
            .saturating_mul(self.link_state_v.saturating_sub(1))
    }
}

fn ser_gateway_mode<S: serde::Serializer>(v: &i32, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(
        GatewayMode::try_from(*v)
            .unwrap_or(GatewayMode::Unspecified)
            .label(),
    )
}

impl ClientAdmission {
    pub fn label(self) -> &'static str {
        match self {
            ClientAdmission::Unspecified => "unspecified",
            ClientAdmission::Admitted => "admitted",
            ClientAdmission::Refused => "refused",
        }
    }
}

fn ser_client_admission<S: serde::Serializer>(v: &i32, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(
        ClientAdmission::try_from(*v)
            .unwrap_or(ClientAdmission::Unspecified)
            .label(),
    )
}

/// True for a region whose members are ROUTERS: `north` on a router, and a
/// gateway's `south:<id>:router`. A `south:<id>:peer` or `:client` region lists
/// peers or clients, which are not routers and are not drawn as routers.
pub fn is_router_region(name: &str) -> bool {
    name == "north" || name.ends_with(":router")
}

#[derive(Clone, PartialEq, ::prost::Message, Serialize)]
pub struct RouterTopologyV1 {
    #[prost(string, tag = "1")]
    pub zid: String,
    #[prost(string, tag = "2")]
    pub site: String,
    #[prost(string, tag = "3")]
    pub node: String,
    #[prost(string, tag = "4")]
    pub zenoh_version: String,

    #[prost(uint64, tag = "5")]
    pub ts_unix_ms: u64,
    #[prost(uint32, tag = "6")]
    pub interval_secs: u32,

    /// Serialized as its LABEL, not its discriminant. The wire form is the
    /// protobuf enum; the JSON form is read by operators, and `1` tells them
    /// nothing while `"auto"` tells them what the router is doing.
    #[prost(enumeration = "GatewayMode", tag = "7")]
    #[serde(serialize_with = "ser_gateway_mode")]
    pub gateway_mode: i32,
    #[prost(message, repeated, tag = "8")]
    pub custom_subregions: Vec<GatewaySubregionV1>,
    #[prost(string, tag = "9")]
    pub gateway_read_error: String,

    #[prost(message, repeated, tag = "10")]
    pub regions: Vec<RegionV1>,
    #[prost(uint32, tag = "11")]
    pub route_successor_count: u32,
    #[prost(message, repeated, tag = "12")]
    pub sessions: Vec<SessionV1>,

    #[prost(string, tag = "13")]
    pub observe_error: String,

    /// Serialized as its label, like `gateway_mode`.
    #[prost(enumeration = "ClientAdmission", tag = "14")]
    #[serde(serialize_with = "ser_client_admission")]
    pub client_admission: i32,
}

impl RouterTopologyV1 {
    pub fn mode(&self) -> GatewayMode {
        GatewayMode::try_from(self.gateway_mode).unwrap_or(GatewayMode::Unspecified)
    }

    /// The next-hop invariant: the route table should be `Σ V(V−1)` over the
    /// regions this router holds. Checked, not assumed (finding T-005).
    pub fn expected_successors(&self) -> u32 {
        self.regions.iter().map(RegionV1::successor_terms).sum()
    }

    pub fn admission(&self) -> ClientAdmission {
        ClientAdmission::try_from(self.client_admission).unwrap_or(ClientAdmission::Unspecified)
    }

    pub fn region(&self, name: &str) -> Option<&RegionV1> {
        self.regions.iter().find(|r| r.name == name)
    }

    pub fn encode_vec(&self) -> Vec<u8> {
        self.encode_to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(name: &str, v: u32, e: u32) -> RegionV1 {
        RegionV1 {
            name: name.into(),
            link_state_v: v,
            link_state_e: e,
            member_zids: vec![],
            session_count: 0,
            ..Default::default()
        }
    }

    #[test]
    fn router_regions_are_north_and_south_router_only() {
        assert!(is_router_region("north"));
        assert!(is_router_region("south:0:router"));
        assert!(!is_router_region("south:0:peer"));
        assert!(!is_router_region("south:1:client"));
    }

    #[test]
    fn links_and_admission_round_trip_and_old_snapshots_decode() {
        let r = RouterTopologyV1 {
            zid: "a1".into(),
            client_admission: ClientAdmission::Refused as i32,
            regions: vec![RegionV1 {
                name: "north".into(),
                link_state_v: 2,
                link_state_e: 1,
                links: vec![LinkV1 {
                    a: "a1".into(),
                    b: "b2".into(),
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let d = RouterTopologyV1::decode(&*r.encode_vec()).expect("decode");
        assert_eq!(d.admission(), ClientAdmission::Refused);
        assert_eq!(d.regions[0].links[0].b, "b2");
        // A snapshot from a publisher that predates both fields: no links, no claim.
        let old = RouterTopologyV1 {
            zid: "c3".into(),
            ..Default::default()
        };
        let d = RouterTopologyV1::decode(&*old.encode_vec()).expect("decode");
        assert_eq!(d.admission(), ClientAdmission::Unspecified);
        let j = serde_json::to_string(&d).unwrap();
        assert!(
            j.contains(r#""client_admission":"unspecified""#),
            "got: {j}"
        );
    }

    #[test]
    fn successor_terms_match_the_m112_measurements() {
        // Three shapes: north V=3 -> 6, V=2 -> 2, V=1 -> 0.
        assert_eq!(region("north", 3, 2).successor_terms(), 6);
        assert_eq!(region("north", 2, 1).successor_terms(), 2);
        assert_eq!(region("north", 1, 0).successor_terms(), 0);
    }

    #[test]
    fn an_edgeless_region_contributes_nothing_however_many_nodes_it_lists() {
        // south:0:peer carries V=3 with NO edges. Counting it predicts 8 against an
        // actual table of 2 and fires a false T-005 on a healthy router.
        let live_zagros = RouterTopologyV1 {
            regions: vec![region("north", 2, 1), region("south:0:peer", 3, 0)],
            route_successor_count: 2,
            ..Default::default()
        };
        assert_eq!(live_zagros.expected_successors(), 2);
        assert_eq!(
            live_zagros.expected_successors(),
            live_zagros.route_successor_count,
            "the live zagros router must not raise T-005"
        );
    }

    #[test]
    fn expected_successors_sums_across_regions() {
        // A test-arm gateway: north V=3 and south:0:router V=3 -> 12.
        let r = RouterTopologyV1 {
            regions: vec![
                region("north", 3, 2),
                region("south:0:router", 3, 2),
                region("south:0:peer", 1, 0),
            ],
            ..Default::default()
        };
        assert_eq!(r.expected_successors(), 12);
    }

    #[test]
    fn an_absent_filter_list_is_not_an_empty_one() {
        // Absent `filters` matches EVERY peer; an empty list matches NONE.
        // Only the second is inert, and conflating them would either miss a
        // real misconfiguration or invent one.
        let absent = GatewaySubregionV1 {
            index: 0,
            filters_present: false,
            filters: vec![],
        };
        let empty = GatewaySubregionV1 {
            index: 0,
            filters_present: true,
            filters: vec![],
        };
        assert!(!absent.is_inert());
        assert!(empty.is_inert());
    }

    #[test]
    fn peer_asserted_axes_are_flagged_and_others_are_not() {
        let by_mode = GatewayFilterV1 {
            uses_modes: true,
            ..Default::default()
        };
        let by_zid = GatewayFilterV1 {
            uses_zids: true,
            ..Default::default()
        };
        let by_region = GatewayFilterV1 {
            uses_region_names: true,
            ..Default::default()
        };
        assert!(!by_mode.uses_peer_asserted_axis());
        assert!(by_zid.uses_peer_asserted_axis());
        assert!(by_region.uses_peer_asserted_axis());
    }

    #[test]
    fn json_shows_the_mode_label_not_its_discriminant() {
        let r = RouterTopologyV1 {
            gateway_mode: GatewayMode::Auto as i32,
            ..Default::default()
        };
        let j = serde_json::to_string(&r).expect("serialize");
        assert!(j.contains(r#""gateway_mode":"auto""#), "got: {j}");
    }

    #[test]
    fn round_trips_through_protobuf() {
        let r = RouterTopologyV1 {
            zid: "a1a1a1a1".into(),
            site: "zagros-sandbox".into(),
            node: "thor-01".into(),
            gateway_mode: GatewayMode::Custom as i32,
            regions: vec![region("north", 3, 2)],
            route_successor_count: 6,
            ..Default::default()
        };
        let decoded = RouterTopologyV1::decode(&*r.encode_vec()).expect("decode");
        assert_eq!(decoded.zid, "a1a1a1a1");
        assert_eq!(decoded.mode(), GatewayMode::Custom);
        assert_eq!(decoded.expected_successors(), 6);
    }
}
