// SPDX-License-Identifier: Apache-2.0
//
// Static server for the Playwright specs: serves ../web/ with the same path
// mapping as the Rust binary's embed.rs (`/` → index.html, `/top`, `/who`,
// `/access` and `/access/identity` → their pages, any other unknown path →
// index.html). The specs mock /api/* and the WebSocket in the browser, so no
// backend runs. Node standard library only.
//
// Usage: node tests/serve.mjs [port]   (default 4173)

import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize, sep } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = fileURLToPath(new URL("../web/", import.meta.url));
const PORT = Number(process.argv[2] ?? 4173);

const PAGES = {
  "": "index.html",
  top: "top.html",
  who: "who.html",
  access: "access.html",
  "access/identity": "access.html",
};

const TYPES = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".css": "text/css",
  ".svg": "image/svg+xml",
  ".json": "application/json",
  ".md": "text/markdown",
};

async function load(rel) {
  const full = normalize(join(ROOT, rel));
  if (!full.startsWith(ROOT.endsWith(sep) ? ROOT : ROOT + sep)) return null;
  try {
    return await readFile(full);
  } catch {
    return null;
  }
}

createServer(async (req, res) => {
  const raw = decodeURIComponent(new URL(req.url ?? "/", "http://x").pathname).replace(/^\/+/, "");
  let rel = PAGES[raw] ?? raw;
  let body = await load(rel);
  if (body === null) {
    rel = "index.html";
    body = await load(rel);
  }
  res.writeHead(200, { "Content-Type": TYPES[extname(rel)] ?? "application/octet-stream" });
  res.end(body);
}).listen(PORT, "127.0.0.1", () => {
  console.log(`serving ${ROOT} on http://127.0.0.1:${PORT}`);
});
