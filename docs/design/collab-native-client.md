# GOAT Collaboration — native client (build spec)

A **parameterizable native app** for the collaboration environment (`collab.example`
and any goatnet's collab plane), for macOS / Windows / Linux (and later iOS / Android).
Exists because Outline's own desktop client is **closed-source and hardwired to
`app.getoutline.com`** (auth-locked; no self-hosted server field — see
[collab-onboarding-provisioner.md] context + Outline discussion #5512), so self-hosters
get no native app. We build our own shell around the web surfaces we already run, take a
**server URL** as config (the thing Outline refused to do), and layer on real native
capabilities the PWA can't reach.

**Status: P1 RELEASED as `v0.1.0` (2026-08-02); P2/P3 pending.** The desktop MVP is
implemented and released from **`goat-collab-app`** (private, org-read) —
Tauri 2 in-window tabbed shell, first-run server picker, one SSO shared across surfaces,
external-link routing, native menu + window-state + single-instance, updater wired to a
signed `latest.json`, and a CI release matrix (mac/win/linux). **macOS is signed +
notarized + stapled** (Developer ID; Gatekeeper-verified); Windows/Linux
ship unsigned (Windows pending a purchased Authenticode cert). The repo's `README`
carries the as-built detail; this doc remains the build spec. Tracking now lives in
in-flight.md (partly-built). P2 (native capabilities +
signed/notarized installers) and P3 (mobile) are the open phases. Read this doc
top-to-bottom, then follow §12 (Execution runbook).

## 1. Goal + non-goals

**Goal.** One installable native app, "GOAT Collaboration", that:
- opens the collab workspace (Outline knowledge base + Files + Zulip chat) behind the
  existing Dex+Kanidm SSO, with one sign-in;
- is pointed at a **configurable server** on first run (default `collab.example`;
  switchable per goatnet — a second goatnet, future spokes) — the core differentiator;
- uses native OS features: notifications, tray/menu-bar, dock/taskbar unread badge,
  deep links, global shortcuts, auto-update, launch-at-login, secure session storage,
  offline/connectivity awareness, native menus + window state.

**Non-goals.**
- NOT re-implementing Outline / Files / Zulip. This is a **shell** around the existing
  web apps (they already run + already do SSO). We add the native layer, not the apps.
- NOT the mesh client. `goat-client` (embeds the netbird fork) is the MESH client and is
  a separate concern (CLAUDE.md "Fleet visibility + clients"). This app touches the
  **collab plane only** (public, Dex/Kanidm) and needs no mesh.
- NOT replacing the PWA. The PWA stays the zero-install option; this is the richer,
  native-capability option for people who want it.

### Integration depth — a tabbed shell, NOT deep federation (decided)

The app spans the whole stack, but at the **shell** level, not by fusing the three
products:

- **One auth is free.** Dex+Kanidm already federates Outline / Files / Zulip — one SSO
  session in the WebView signs you into all three. The app inherits it via the shared
  cookie jar; there is **nothing to build** for "our auth across the stack."
- **Target: a native tabbed shell.** Outline / Files / Chat as native tabs (or a left
  rail) in one window, each its own WebView sharing the SSO session, plus the native
  *unifiers*: one OS-notification stream across all three, a single **summed** unread
  badge, and `goatcollab://` deep links that route to the right tab (using the
  cross-surface deep-link contract, [zenoh-surface-deep-links.md]). Feels like one
  product — one login, one window, one notification model — for ~20% of the cost of true
  integration, while each surface stays its own robust web UI.
- **Explicitly deferred: deep federation.** Unified *search* across chat+docs+files, a
  *merged sidebar* of channels+docs+files, and cross-surface *actions* (drag a file into
  a chat, inline-preview a Files doc in Zulip) all require an aggregation layer over three
  independent products' APIs (Zulip, Outline, our filedrop) — product-scale work, fragile
  as those APIs change, uncertain payoff. Leave hooks; add selectively later on demand
  (cheapest first taste = a global quick-switcher / command palette). Do NOT build the
  fused UI in v1.

## 2. Tech decision — Tauri 2 (committed)

**Use Tauri 2**, not Electron.
- Cross-platform desktop **and mobile** from one codebase (Tauri 2 ships iOS + Android) —
  covers "all platforms" without a second stack.
- Rust core → fits the repo's existing Rust expertise (`goat-cli`, `goat-relay`); the
  native layer is Rust, the UI is the system WebView (WKWebView / WebView2 / WebKitGTK).
- Small binaries (~3–10 MB vs Electron's ~100 MB), smaller attack surface, per-command
  capability allow-listing (good fit for our security posture).
- First-class updater, notification, tray, deep-link, global-shortcut, single-instance
  plugins.

Trade-off: system WebView means per-platform rendering differences (WebKit vs Chromium).
Acceptable — Outline / Zulip web already target broad browsers, and we control the CSS.

## 3. Architecture

```
┌──────────────────────────────────────────────────────────┐
│  GOAT Collaboration.app  (Tauri 2)                         │
│                                                            │
│  ┌────────────────────────┐   ┌───────────────────────┐   │
│  │  System WebView        │   │  Native core (Rust)   │   │
│  │  loads https://<server>│◀──│  - server-URL config  │   │
│  │  (SSO happens here,    │   │  - tray / menu / badge│   │
│  │   cookie session,      │   │  - notifications      │   │
│  │   exactly like the PWA)│   │  - deep links (scheme)│   │
│  │                        │──▶│  - global shortcuts   │   │
│  │  injected JS bridge:   │   │  - auto-update        │   │
│  │  unread counts, title, │   │  - launch-at-login    │   │
│  │  desktop-notify calls  │   │  - offline detect     │   │
│  └────────────────────────┘   └───────────────────────┘   │
└──────────────────────────────────────────────────────────┘
```

- **WebView loads `https://<server>/`** (the collab-home splash → tiles → sub-apps). The
  ALB oauth2-proxy gate → Dex → Kanidm login happens IN the WebView, cookie-based, the
  same flow the PWA uses. No native OIDC needed for v1 (see §6).
- **JS↔Rust bridge** (`@tauri-apps/api` + injected script): the web side reports unread
  counts / titles / notify requests; the Rust side renders them natively (badge, tray,
  OS notification). A small injected `init.js` polls the DOM (Zulip unread, Outline
  title) OR — better — subscribes to what the apps already expose (Zulip has a JS API;
  Outline emits title changes). Start with title/badge; deepen per-app over time.
- **External links** (non-collab domains) open in the system browser, not in-app.

## 4. Native capabilities to build (the "take advantage of native" list)

Phase them (§11), but the target set:

| Capability | How (Tauri 2) | Value |
|---|---|---|
| **Server URL config** | first-run form → stored in app config dir; menu "Switch server" | works for any goatnet's collab env (Outline can't) |
| **Multi-goatnet** | list of servers; window per server or switcher | one app across collab.example / other goatnets / spokes |
| **Native notifications** | `notification` plugin; web calls through the bridge | chat mentions, doc comments, without the tab open |
| **Dock/taskbar unread badge** | `app.setBadgeCount()` / tray title | glanceable unread |
| **Tray / menu-bar icon** | `tray-icon` plugin | quick open, presence, quit-to-tray |
| **Deep links / custom scheme** | `deep-link` plugin, `goatcollab://` | open a specific doc/channel; ties into the cross-surface deep-link contract ([zenoh-surface-deep-links.md]) |
| **Global shortcut** | `global-shortcut` plugin | quick-toggle the window |
| **Auto-update** | `updater` plugin + signed manifest | keep the fleet current (§9) |
| **Launch at login** | `autostart` plugin | always-on chat |
| **Secure storage** | `stronghold` / OS keychain | remember server + session hints (NOT passwords — SSO owns those) |
| **Offline / connectivity** | Rust net probe → banner | honest "you're offline" vs "server down" |
| **Native menus + window state** | Tauri menu + `window-state` plugin | real app-menu, remembered size/position |
| **Single instance** | `single-instance` plugin | one app, focus-existing |

## 5. Server-URL parameterization (the core differentiator)

- **First run:** a small native screen — "Enter your GOAT Collaboration server" — default
  `collab.example`, validated by hitting `https://<server>/healthz`-ish (the collab-home
  serves `/healthz`) before accepting.
- **Stored** in the app config dir (`$APPCONFIG/goat-collab/servers.json`): list of
  `{name, url}`; the active one loads at launch.
- **Switchable** from the app menu ("Servers ▸ Add / Switch"). Each goatnet's collab env
  is just another entry — another goatnet, future spokes, a partner's standalone goatnet.
- This is the whole point: Outline hardcodes `app.getoutline.com`; we take a URL. It also
  makes the app **turnkey per goatnet** — ship one binary, every collab env can use it.

## 6. SSO / auth

**v1: WebView cookie session (no native OIDC).** Load `https://<server>/`; the ALB
authenticate-oidc + oauth2-proxy → Dex → Kanidm flow runs in the WebView exactly like a
browser. The session cookie lives in the WebView's cookie jar (persisted per platform).
This is the lowest-friction path and mirrors the PWA — ship it first.

Gotchas to handle:
- **OAuth redirects to `auth.collab.example` / `idm.collab.example`** stay in-app (same
  collab origin family) — don't punt them to the system browser. Only *foreign* domains
  open externally.
- **Passkey/WebAuthn in the WebView:** WKWebView + WebView2 support WebAuthn; verify per
  platform. Password+TOTP always works. (Kanidm MFA is required — §collab MFA policy.)
- **Cookie persistence** across launches: enable per-platform (WKWebsiteDataStore /
  WebView2 UserDataFolder / WebKitGTK persistent cookies).

**v2 (optional):** native OIDC PKCE via `goatcollab://` redirect + system browser, if we
want token-based native API calls (e.g., native notification polling without a live
WebView). Not needed for v1.

## 7. Repo + project structure

**New sibling repo: `goat-collab-app`** (private + org-read, like the other
usareur-af sibling repos — goat-partner-showcase, moon-pod). Rationale: it's a
distributable client with its own release/CI/signing lifecycle; keeping it out of
`collab-services` (Terraform/infra) and out of `goat` (mesh) keeps concerns
clean. It has no mesh dependency.

```
goat-collab-app/
  src-tauri/            # Rust native core
    src/main.rs         # window, plugins, bridge commands
    src/servers.rs      # server-URL config (load/save/validate)
    src/native.rs       # tray, badge, notifications, deep-link, shortcuts
    tauri.conf.json     # app id com.usareur-af.goatcollab, updater, capabilities
    capabilities/       # per-command allow-list
    icons/              # generated (tauri icon)
  src/                  # tiny web front-end (first-run + server-switcher UI only;
                        #   the collab apps themselves are remote)
    index.html          # first-run / server picker (styled like collab-home)
    inject.js           # injected into the remote page: unread/title → bridge
  .github/workflows/
    release.yml         # tauri-action matrix build + sign + publish (§8/§9)
  README.md
```

App identity: name **"GOAT Collaboration"**, bundle id **`com.usareur-af.goatcollab`**,
scheme **`goatcollab://`**.

## 8. CI build matrix (all platforms)

GitHub Actions with `tauri-apps/tauri-action`, matrix:
- **macOS** (`macos-latest`): universal `.dmg` + `.app` (aarch64 + x86_64).
- **Windows** (`windows-latest`): `.msi` + NSIS `.exe`.
- **Linux** (`ubuntu-latest`): `.AppImage` + `.deb` (+ optionally `.rpm`).

Triggered on tag `v*`. Produces GitHub Releases assets + the signed update manifest
(`latest.json`) for the updater.

**Mobile (Phase 3):** Tauri 2 iOS (`.ipa`, Apple Developer + provisioning) + Android
(`.apk`/`.aab`, keystore). Separate workflows; store review out of scope for v1.

## 9. Signing / notarization (the genuinely hard part — call it out)

This is where the effort is, not the code. Prereqs (secrets, operator-provided):
- **macOS:** Apple **Developer ID Application** cert (.p12) + an **App-Store-Connect API
  key** for `notarytool`; hardened runtime + notarize + staple in CI. **Reuse the
  approach in [goat-cli-macos-signing.md]** — we already fought this for the CLI;
  same certs/secrets/notary flow. Without notarization, macOS Gatekeeper blocks it.
- **Windows:** a **code-signing certificate** (OV/EV; EV avoids SmartScreen warnings).
  Sign the `.msi`/`.exe` in CI. Unsigned → SmartScreen "unknown publisher".
- **Linux:** AppImage/deb unsigned is normal; optionally GPG-sign the deb repo.

Updater signing: Tauri's own **minisign** key pair (public key baked into
`tauri.conf.json`, private key a CI secret) signs `latest.json` + artifacts — separate
from OS code-signing, required for auto-update integrity.

**These certs/keys are prerequisites the operator must supply before a signed release.**
Unsigned dev builds can be produced without them for testing.

## 10. Auto-update + distribution

- **Auto-update:** Tauri `updater` plugin checks a hosted `latest.json` (minisign-signed);
  on a new version it downloads + installs. Host `latest.json` + assets on **GitHub
  Releases** (simplest) and/or the collab env's `get.<site>` surface (matches
  [goat-cli-mesh-download.md] — but the collab client is PUBLIC, so public GitHub Releases
  is fine and simpler than the mesh get surface).
- **Distribution / discovery:** add a real **"Download the app"** section to the
  collab-home splash (`collab-services/services/collab-home/index.html`) with per-OS
  download links — this is the fix for the very confusion that started this (Outline has
  no self-hosted client; ours does). Detect OS in JS, show the right asset.
- **Versioning + release runbook:** mirror [goat-cli-release.md] (tag → CI builds+signs →
  Release → bump the download links). Keep a deploy-history-style ledger if we pin it
  anywhere.

## 11. Phases

- **P1 — desktop MVP (mac/win/linux): BUILT (2026-07-31, `goat-collab-app`).**
  Tauri shell with a **native tabbed layout** (Outline / Files / Chat, each a WebView
  sharing one SSO cookie jar — see §1 "Integration depth"; ships collab-home as tab 0 with
  explicit per-surface tabs alongside), first-run server-URL config, cookie SSO end-to-end
  (one login → all three), external-link handling, native menu + window state, auto-update
  wired, **unsigned dev builds** for internal test. Acceptance: an internal tester installs
  it, points at `collab.example`, signs in ONCE, and uses all three surfaces from the tabs
  — the sign-in leg is a human-tester step (needs real Kanidm creds); everything up to the
  login wall is verified by `cargo test` + the release-build pipeline. **One deliberate
  scope cut:** the summed unread badge moved to P2 — it needs remote→core IPC, which Tauri's
  compile-time-static capability allowlist can't grant for an arbitrary server URL; P1 puts
  the active surface in the window title and leaves `inject.js` as the ready seam.
- **P2 — native capabilities + signed release:** notifications + dock badge + tray +
  deep links (`goatcollab://`) + global shortcut + launch-at-login + offline banner;
  **signed + notarized** macOS/Windows builds; download links on collab-home; auto-update
  live. Acceptance: signed installers pass Gatekeeper/SmartScreen; chat mention fires an
  OS notification with the app in the tray; updater upgrades a running install.
- **P3 — mobile (iOS/Android):** Tauri 2 mobile targets; store/provisioning; push (later).

## 12. Execution runbook (for the one-shot session)

1. **Create the repo** `goat-collab-app` (private, org-read). `gh repo create`.
2. **Scaffold** Tauri 2: `pnpm create tauri-app` (vanilla TS front-end) → set app id
   `com.usareur-af.goatcollab`, name "GOAT Collaboration", scheme `goatcollab://`.
3. **Server-URL config** (`src-tauri/src/servers.rs` + first-run `src/index.html`): default
   `collab.example`, validate via `/healthz`, persist to app config dir, menu switcher.
4. **Load the remote** collab env in the main window; wire external-link + OAuth-redirect
   handling (§6 gotchas); enable persistent cookies per platform.
5. **Bridge + inject.js:** report title/unread → Rust; render dock badge + tray.
6. **Native plugins:** notification, tray-icon, deep-link, global-shortcut, single-instance,
   autostart, window-state, updater. Allow-list each in `capabilities/`.
7. **CI** (`.github/workflows/release.yml`): `tauri-action` matrix (mac/win/linux) on
   `v*`; produce unsigned dev builds first.
8. **Signing** (P2): add the macOS Developer-ID + notary secrets (reuse F-208 flow),
   Windows cert, minisign updater key; flip CI to signed + notarized.
9. **Distribution:** add the "Download the app" section to collab-home
   (`collab-services` — note: needs a Docker/CI image rebuild, or add collab-services CI
   first — see the open item in the platform report) + GitHub Releases.
10. **Verify** the P1/P2 acceptance criteria; write a short release runbook mirroring
    goat-cli-release.md.

## 13. Prerequisites the operator must supply (blockers for a *signed* release)

- Apple Developer ID Application cert (.p12) + App-Store-Connect API key (notary).
- Windows code-signing cert (EV preferred).
- Decision: repo name/visibility (`goat-collab-app` assumed).
- Decision: does v1 wrap the whole workspace (collab-home tiles) or open Outline
  directly? (Assumed: whole workspace — one app, SSO, all three surfaces.)

Unsigned dev builds (P1) need none of these — start there.

## 14. Open questions (resolve in the build session or with the operator)

- Per-app unread signals: Zulip exposes unread via its web API / title; Outline via title;
  Files has none. How deep to integrate vs. just title-based badge for v1 (start title-based,
  summed across tabs).
- Tabbed-shell vs single-webview: **DECIDED** — native tabbed shell (Outline/Files/Chat as
  tabs, each a WebView sharing the SSO), not a single WebView to collab-home, and not a
  fused UI. See §1 "Integration depth". (P1 may start with the collab-home landing as tab 0
  and add the explicit per-app tabs as the first P1 increment.)
- Mobile push (P3) would need a push gateway (the mesh has hedwig/UnifiedPush for Matrix;
  Zulip has its own push) — defer.
