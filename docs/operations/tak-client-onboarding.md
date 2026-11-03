# TAK Client Onboarding

How to connect ATAK, WinTAK, iTAK, and WebTAK clients to the goat TAK Server over the mesh.

**Prerequisites for all clients:**
1. Device is enrolled as a Netbird mesh peer (or connected to a mesh-connected network)
2. Client certificate generated: `ops/tak/enroll-client.sh <username>`
3. TAK Server running: verify with `curl -sk https://tak.netbird.legacy.example:8443/`

---

## Generate client credentials

From your admin machine (on the mesh):

```
ops/tak/enroll-client.sh <username>
```

This creates two files in `ops/tak/certs/`:
- `<username>.p12` — client certificate (for manual import)
- `<username>-dp.zip` — ATAK data package (cert + server connection profile, auto-import)

---

## ATAK (Android)

Primary mobile SA client. Install from tak.gov (government) or Google Play (civilian).

**Method 1: Data package (recommended)**

1. Transfer `<username>-dp.zip` to the Android device (ADB push, file share, email, USB)
2. Open ATAK
3. Import Manager (hamburger menu → Import Manager)
4. Select the .zip file
5. ATAK imports the certificate and server connection automatically
6. Blue dot appears on the map — you're connected

**Method 2: Manual configuration**

1. Transfer `<username>.p12` to the device
2. ATAK → Settings → Network Preferences → Manage Server Connections
3. Add connection:
   - Description: `goat TAK`
   - Address: TAK host mesh IP (or `tak.netbird.legacy.example` if DNS works on device)
   - Port: `8089`
   - Protocol: `SSL`
   - Client cert: select `<username>.p12`
   - CA cert: select `truststore-goatCA.p12`
4. Enable the connection

**Verify:** Your position appears as a blue icon on the map. Other connected users' positions appear. Chat is available.

---

## WinTAK (Windows)

Desktop SA client for command posts and rear operations. Install from tak.gov.

1. Ensure Netbird is running on the Windows machine (mesh peer)
2. Transfer `<username>-dp.zip` to the machine
3. WinTAK → Settings → Network → Import
4. Select the .zip file
5. WinTAK imports the cert and server connection

**Or manually:**

1. Transfer `<username>.p12` to the machine
2. WinTAK → Settings → Network → Manage Server Connections
3. Add server:
   - Address: TAK host mesh IP
   - Port: `8089`
   - Protocol: `SSL`
   - Import client cert and CA truststore

---

## iTAK (iOS)

iOS SA client. Install from the App Store.

1. Ensure the iOS device is on the mesh:
   - Option A: Install Netbird iOS client, enroll with setup key
   - Option B: Connect to WiFi on a mesh-connected network (e.g., router running Netbird)
2. Transfer `<username>.p12` to the device (AirDrop, email, Files app)
3. iTAK → Settings → Servers → Add
4. Configure server address, port 8089, SSL
5. Import the client certificate

---

## WebTAK (Browser)

Browser-based COP. No app install needed — any mesh peer with a browser.

1. Ensure the machine is a Netbird mesh peer
2. Import `<username>.p12` into the browser's certificate store:
   - **Chrome/Edge (macOS):** Double-click the .p12 → Keychain Access imports it
   - **Chrome/Edge (Windows):** Settings → Privacy and Security → Manage certificates → Import
   - **Firefox:** Settings → Privacy & Security → View Certificates → Your Certificates → Import
3. Also import `truststore-goatCA.p12` as a trusted CA:
   - **macOS Keychain:** Double-click → mark as "Always Trust"
   - **Windows:** Import to "Trusted Root Certification Authorities"
   - **Firefox:** Authorities tab → Import → check "Trust this CA to identify websites"
4. Navigate to: `https://tak.netbird.legacy.example:8443/webtak/index.html`
5. Browser prompts to select a client certificate — choose `<username>`
6. Full COP loads in the browser

**Troubleshooting:**
- Certificate prompt doesn't appear → verify the .p12 was imported and browser was restarted
- Connection refused → verify Netbird is running (`sudo netbird status`)
- TLS error → verify the CA truststore is imported as a trusted authority

---

## TAK ICU (Android — Video Streaming)

Streams video from an Android camera into TAK Server. Other clients see the feed on the map.

1. Install TAK ICU from tak.gov
2. Ensure the device is a mesh peer (Netbird running)
3. Configure TAK ICU:
   - Server: TAK host mesh IP
   - Port: 8089
   - Import client certificate
4. Start streaming — video feed appears in ATAK/WinTAK/WebTAK

Requires the Video plugin enabled on TAK Server (enable in admin UI at port 9000).

---

## Enrollment via browser (alternative)

TAK Server's enrollment port (8446) allows certificate provisioning without admin-side scripting:

1. Admin creates a user account in the TAK Server admin interface (port 9000)
2. User navigates to `https://tak.netbird.legacy.example:8446`
3. User logs in with username/password
4. User downloads their .p12 certificate and connection profile
5. User imports into their TAK client

This is useful for self-service onboarding when you don't want to run `enroll-client.sh` for each user.

---

## Netbird enrollment reminder

Every TAK client device needs to be on the mesh first. One command:

```
sudo netbird up --management-url https://netbird.legacy.example --setup-key $NETBIRD_SETUP_KEY
```

Once enrolled, the device can reach `tak.netbird.legacy.example` over WireGuard. The TAK connection rides inside the encrypted tunnel — no additional network config needed.
