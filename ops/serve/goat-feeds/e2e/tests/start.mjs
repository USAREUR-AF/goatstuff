// SPDX-License-Identifier: Apache-2.0
// Starts goat-feeds for the browser tests: a fresh database, a config with documentation
// addresses only, and the stub pipe. Run by playwright.config.ts as its web server.
import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const port = process.argv[2] || "18199";
const dsn = process.env.GOAT_FEEDS_E2E_DSN;
if (!dsn) {
  console.error("start.mjs: set GOAT_FEEDS_E2E_DSN to a Postgres database the tests may wipe");
  process.exit(2);
}
const bin = resolve(process.env.GOAT_FEEDS_BIN || join(here, "..", "..", "goat-feeds"));
if (!existsSync(bin)) {
  console.error(`start.mjs: no binary at ${bin}; build it first (go build -o goat-feeds .)`);
  process.exit(2);
}
// Each run starts empty. psql is on CI runners; when it is absent the database must be fresh.
const wipe = spawnSync("psql", [dsn, "-q", "-c", "DROP TABLE IF EXISTS feeds, feed_events"], { stdio: "inherit" });
if (wipe.error) console.error("start.mjs: psql not found; assuming a fresh database");

const cfg = {
  site: "test-site",
  goatnet: "test-goatnet",
  listen: `127.0.0.1:${port}`,
  tactical_groups: ["TACTICAL-ADMIN"],
  platform_groups: ["PLATFORM-ADMIN"],
  allowed_cidr: "198.51.100.0/24",
  pipe: { binary: join(here, "fake-pipe.sh"), packs: "/packs", zenoh_config: "/zenoh.json5" },
  catalog: [
    { id: "fused", label: "Fused air picture", key: "slot/stream/fused/tracks", kind: "processed", data_class: "real",
      from_format: "goat-processors-system-track", in_frame: "newline", description: "Correlated, fused and filtered tracks.",
      outputs: [{ id: "cat062", label: "ASTERIX CAT062 (SAC 38 / SIC 204)", to_format: "asterix-cat062", set: { "cat062.sac": "38", "cat062.sic": "204" } },
                { id: "sapient", label: "SAPIENT", to_format: "sapient" }] },
    { id: "sim", label: "Fused air picture (simulated)", key: "slot/stream/sim/tracks", kind: "processed", data_class: "synthetic",
      from_format: "goat-processors-system-track", outputs: [{ id: "cat062", label: "ASTERIX CAT062 (SAC 0 / SIC 1)", to_format: "asterix-cat062" }] },
    { id: "radar", label: "Radar tracks (raw)", key: "slot/radar/tracks", kind: "raw", data_class: "real",
      from_format: "cot-xml", outputs: [{ id: "cat062", label: "ASTERIX CAT062", to_format: "asterix-cat062" }] },
  ],
  intake: {
    listen_ip: "198.51.100.9", ports: [7100, 7101, 7102, 7103], key_prefix: "volatile/slot/stream/feeds-in",
    formats: [{ id: "mavlink2", label: "MAVLink 2", from_format: "mavlink2", note: "Drone autopilot position" },
              { id: "cot", label: "Cursor on Target (CoT XML)", from_format: "cot-xml" }],
    outputs: [{ id: "cot", label: "CoT XML", to_format: "cot-xml", frame: "newline" },
              { id: "cat062", label: "ASTERIX CAT062 (SAC 38 / SIC 210)", to_format: "asterix-cat062" }],
  },
};
const dir = mkdtempSync(join(tmpdir(), "goat-feeds-e2e-"));
// Declared links (ADR 1260): one manifest, read through fake systemctl/journalctl.
const declDir = join(dir, "declared.d");
mkdirSync(declDir);
writeFileSync(join(declDir, "pipeline-test.json"), JSON.stringify({ owner: "processors pipeline test", links: [
  { id: "fused-to-console", unit: "goat-xf-pipe@fused-test.service", direction: "out", source_key: "slot/stream/fused/tracks",
    source_label: "Fused air picture (declared)", sources: [{ label: "Radar A", site: "alpha", system: "radar-a" }, { label: "Radar B", site: "alpha", system: "radar-b" }], to_format: "asterix-cat062", output_label: "ASTERIX CAT062 (SAC 38 / SIC 204)",
    dest: "udp://198.51.100.70:7005", data_class: "real", controlled_by: "destinations.test.enabled in the pipeline declaration", enabled: true },
] }));
cfg.declared_dir = declDir;
// Facets: a small estate, one console declared in the settings, and sources on the raw topic.
const estatePath = join(dir, "estate.json");
writeFileSync(estatePath, JSON.stringify({
  regions: [{ region_id: "north", label: "North" }],
  sites: [{ site_id: "alpha", label: "Alpha", region_id: "north", systems: [
    { name: "radar-a", class: "radar" }, { name: "radar-b", class: "radar" }, { name: "radar-c", class: "radar", duty_cycled: true }] }],
}));
cfg.estate_file = estatePath;
cfg.systems = [{ name: "console-70", label: "Console 70", site: "alpha", class: "console", peer: "198.51.100.70" }];
for (const t of cfg.catalog) if (t.id === "radar") t.sources = [{ label: "Radar C", site: "alpha", system: "radar-c" }];
cfg.declared_probe = { systemctl: join(here, "fake-systemctl.sh"), journalctl: join(here, "fake-journalctl.sh") };
const cfgPath = join(dir, "config.json");
writeFileSync(cfgPath, JSON.stringify(cfg, null, 2));
const child = spawn(bin, ["-config", cfgPath], { stdio: "inherit", env: { ...process.env, GOAT_FEEDS_DB_DSN: dsn } });
const stop = () => child.kill("SIGTERM");
process.on("SIGTERM", stop);
process.on("SIGINT", stop);
child.on("exit", (code) => process.exit(code ?? 0));
