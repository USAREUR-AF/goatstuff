# edge-model-events: a model at the edge publishes what it sees

A camera on a factory line reads barcodes and QR codes and recognises objects. Each thing it sees is published once, as it happens, as an event on the fabric. The event goes out under the publisher's own slot, so a subscriber at another site knows about it straight away and can tell where it came from.

This is a worked example for the goat gym module *Publish what a model at the edge sees, as it happens*. The "model" replays a fixture of synthetic detections and recognises nothing. `detections()` in `edge-model-events.py` is the one function a real detector replaces. Everything else is the part to copy.

## What it does

| Choice | What the example does | Why |
|---|---|---|
| Key | One key per source: `<slot>/edge/<site>/<source>/detections/v1` | The site and the source are small, bounded sets a subscriber may want on their own, so they are key segments: `<slot>/edge/site-a/**` follows one site. Every distinct key is a routing entry. A key per event grows the router's table without bound and slows the fabric for everyone (goatcore `pubsub-best-practices.md`, "The stable-key contract"). |
| Payload | JSON with `source`, `site`, `seq`, `seen_at` (UTC), `kind`, `value` and `confidence` | Everything that varies per event goes here, never in the key. |
| Slot | The key starts with your slot id or its alias | Your certificate may write only under your slot. A publish outside it is denied silently. |
| Class | A normal prefix, not `volatile/` | A sighting is a record someone may need to replay. A `volatile/` key is routed but never stored or audited (goatcore `zenoh-volatile-class.md`). |
| Transport | `goat pub --encoding json`, once per event | `goat pub` carries your identity and certificate. One process per event suits a few events a second. A higher rate needs one Zenoh session with a declared publisher. |
| Rate | Declared as a throughput envelope on your registration in the portal, before you start | A detector is event-driven, so its floor is 0: a quiet line is not a fault. Its maximum says what a burst may reach (goatcore `qos-throughput-envelopes.md`). |

## Run it

Offline, to see every publish without running any:

```bash
python3 ops/zenoh-trial-peers/edge-model-events/edge-model-events.py \
  --slot <your-slot> --source line-3-camera --site site-a --dry-run
```

On a goatnet, first start the subscriber at another site and leave it running. Live pub/sub keeps no backlog, so a subscriber that starts late misses the events.

```bash
goat sub '<your-slot>/edge/site-a/line-3-camera/detections/v1' --take 6 --for 5m
```

Then, from the machine the model runs on, enrolled with the goat CLI under your slot:

```bash
python3 ops/zenoh-trial-peers/edge-model-events/edge-model-events.py \
  --slot <your-slot> --source line-3-camera --site site-a
```

The subscriber prints six events, numbered `seq` 1 to 6, matching the lines the publisher printed. A `goat pub` that exits 0 is not proof the event arrived; the subscriber is.

## Tests

```bash
python3 -m pytest -q ops/zenoh-trial-peers/edge-model-events
```

The tests pin what the example teaches:
- one stable key under the slot for every event;
- the payload carries what varies;
- a placeholder slot, or a source that would change the key, never reaches the wire;
- a failed publish stops the run.

They do not touch a fabric. The live publish and subscribe have not been run from this example.
