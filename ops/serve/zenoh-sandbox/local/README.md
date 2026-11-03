# Zenoh sandbox — offline / field demo (macOS / Linux)

> **Windows?** Use the Docker-based dev compose instead — no Node or platform
> binary required:
> `docker compose -f ops/serve/zenoh-sandbox/docker-compose.dev.yml up --build`
> then open `http://localhost:8099`. See `docs/design/zenoh-sandbox.md` §"Local dev".

Run the whole sandbox on one laptop with **no internet and no Docker** — for
demoing somewhere with no connection. Everything that needs the network is done
once, ahead of time, by `prep.sh`; `serve.sh` then runs fully offline.

## How it works

The sandbox is just two pieces: the static web app, and a local Zenoh router
with the `remote_api` WebSocket plugin (the `zenoh-bridge-remote-api` binary).
`serve.sh` runs the router on `:10000` and serves the prebuilt app on `:8080`;
the app's default server URL on `localhost` is already `ws://localhost:10000`,
so it just connects. No mesh, no nova-a, no internet — the same isolated,
disposable model as `sandbox.nova`, on your machine.

## One-time prep (needs internet)

```bash
cd ops/serve/zenoh-sandbox/local
./prep.sh
```

This builds `../app/dist` (Node 20+) and downloads the matching
`zenoh-bridge-remote-api` for your OS/arch into `./bin/` (both are gitignored).
Do this before you leave connectivity. To prep for a *different* target machine,
build there too, or copy the produced `../app/dist` + `./bin/` across.

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
  preinstalled on most Linux). The static server sets the correct
  `application/wasm` MIME the zenoh-ts client needs.
- **No Docker** — deliberately. Docker Desktop is heavy; this kit is a couple of
  native processes.
- Showing it on a projector via another device on the same LAN? Serve still
  binds localhost; point that device's app at `ws://<this-host>:10000` by typing
  it in the server-URL field.
- This is the same app shipped to `sandbox.nova` (`docs/design/zenoh-sandbox-ux.md`);
  only the delivery differs.
