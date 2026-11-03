//! Configuration, entirely from the environment.
//!
//! Every value is `NETVIS_*` — its own namespace, because this is its own
//! service. Reusing `MICROSCOPE_*` would have quietly coupled the two: a
//! change to microscope's token or fence would have moved netvis with it, which
//! is precisely the coupling that splitting the service was meant to remove.

pub struct Config {
    pub listen: String,
    pub site_label: String,
    pub netbird_url: String,
    pub netbird_token: String,
    /// `off` serves to anyone who reaches the port — for local inspection only.
    /// Anything else enforces `rbac_groups` against the proxy's groups header.
    pub rbac_mode: String,
    pub rbac_groups: String,
    /// The portal that answers "who owns this peer"
    /// (`POST /api/access/peers`, docs/design/peer-identity-resolution.md).
    /// Empty disables the ownership lens — every peer then reports
    /// `attribution: unread`, which is the honest value for "we did not look".
    /// It never blanks the field, because a blank reads as "nobody owns this".
    pub portal_url: String,
    /// netvis's OWN portal key, not microscope's. Same reasoning as the
    /// NETVIS_* namespace above: a shared credential means revoking one
    /// surface's access silently revokes the other's, and the two have
    /// different fences and different release cadences.
    pub portal_api_key: String,
    /// Base URL of the id-admin-console, for the resolved-person deep link
    /// (`admin.auth.<site>.<zone>`). Supplied by the server rather than
    /// guessed by the browser from its own hostname: the console's host is a
    /// per-goatnet fact that lives in group_vars, and a link derived by
    /// string-munging `location.hostname` is one rename away from 404ing on
    /// every row while looking perfectly fine in review.
    pub id_console_url: String,
}

impl Config {
    pub fn from_env() -> Self {
        let get = |k: &str| std::env::var(k).unwrap_or_default();
        let or = |k: &str, d: &str| {
            let v = get(k);
            if v.is_empty() { d.to_string() } else { v }
        };
        Self {
            listen: or("NETVIS_LISTEN", "0.0.0.0:9760"),
            site_label: or("NETVIS_SITE_LABEL", "goatnet"),
            netbird_url: get("NETVIS_NETBIRD_URL"),
            netbird_token: get("NETVIS_NETBIRD_TOKEN"),
            // Fail closed by default. A surface that renders the whole ACL is
            // the wrong place for a permissive default.
            rbac_mode: or("NETVIS_RBAC_MODE", "enforce"),
            // The matrix vocabulary (ADR 1073), same door as the other
            // admin-gated surfaces.
            rbac_groups: or("NETVIS_RBAC_GROUPS", "PLATFORM-ADMIN,IAM-ADMIN"),
            // Both default empty: the lens is off until a goatnet wires it,
            // and off is a state the payload states rather than hides.
            portal_url: get("NETVIS_PORTAL_URL"),
            portal_api_key: get("NETVIS_PORTAL_API_KEY"),
            // Empty renders the person as plain text instead of a link. A
            // resolved owner with no link is still the answer to "who do I
            // ask"; a link to the wrong host is worse than none.
            id_console_url: get("NETVIS_ID_CONSOLE_URL"),
        }
    }
}
