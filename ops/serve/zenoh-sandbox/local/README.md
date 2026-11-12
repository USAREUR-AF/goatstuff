# Zenoh sandbox — offline / field demo (macOS / Linux)

> **Windows?** Use the Docker-based dev compose instead — no platform
> binary required:
> `docker compose -f ops/serve/zenoh-sandbox/docker-compose.dev.yml up --build`
> then open `http://localhost:8099`. See `docs/design/zenoh-sandbox.md` §"Local dev".

Run the whole sandbox on one laptop with **no internet and no Docker** — for
demoing somewhere with no connection. Everything that needs the network is done
once, ahead of time, by `prep.sh`; `serve.sh` then runs fully offline.

## How it works

The sandbox is just two pieces: the static web app, and a local Zenoh router
with the `remote_api` WebSocket plugin (the `zenoh-bridge-remote-api` binary).
`serve.sh` runs the router on `:10000` and serves `../app` on `:8080`;
the app's default server URL on `localhost` is already `ws://localhost:10000`,
so it just connects. No mesh, no nova-a, no internet — the same isolated,
disposable model as `sandbox.nova`, on your machine.

## One-time prep (needs internet)

```bash
cd ops/serve/zenoh-sandbox/local
./prep.sh
```

This downloads the matching `zenoh-bridge-remote-api` for your OS/arch into
`./bin/` (gitignored). Do this before you leave connectivity. The web app needs
no preparation: `../app` is static files, served as committed. To prep for a
*different* target machine, run `prep.sh` there, or copy this checkout with a
`./bin/` that holds the router for that machine's OS/arch.

## At the demo (no internet)

```bash
cd ops/serve/zenoh-sandbox/local
./serve.sh
```

Opens `http://localhost:8080`. Click **Connect**, start the 🌡 sensor, **Listen**
on `demo/**`, run a **Transform**, and watch packets cross the brokerless
topology. Ctrl-C stops it. Ports override via `WEB_PORT` / `WS_PORT`.

## Notes

- **python3** is the only runtime dependency at demo time (ships with macOS;
  preinstalled on most Linux). `static-server.py` binds to localhost and tells
  the browser not to cache.
- **No Docker** — deliberately. Docker Desktop is heavy; this kit is a couple of
  native processes.
- Showing it on a projector via another device on the same LAN? Serve still
  binds localhost; point that device's app at `ws://<this-host>:10000` by typing
  it in the server-URL field.
- This is the same app shipped to `sandbox.nova` (`docs/design/zenoh-sandbox-ux.md`);
  only the delivery differs.
