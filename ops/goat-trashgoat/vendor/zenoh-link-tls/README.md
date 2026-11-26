# zenoh-link-tls 1.10.2, patched

Copy of the published crate `zenoh-link-tls` **1.10.2** (Eclipse Zenoh, `EPL-2.0 OR Apache-2.0`, Copyright ZettaScale Technology), taken from crates.io. Workspaces pick it up through:

```toml
[patch.crates-io]
zenoh-link-tls = { path = "<relative path>/vendor/zenoh-link-tls" }
```

The patch replaces only `zenoh-link-tls`. Every other zenoh crate stays on the published `=1.10.2`.

## Diff from upstream 1.10.2

| File | Change |
|---|---|
| `src/unicast.rs` | The TLS stream moves from an `UnsafeCell` to a `std::sync::Mutex` held for one `poll_read` / `poll_write` / `poll_flush` / `poll_shutdown` call and never across an `await`, the scheme `tokio::io::split` uses. `read`, `read_exact`, `write`, `write_all` and `close` go through it. `Drop` uses `Mutex::get_mut`. The `unsafe impl Send/Sync` is removed because every field is `Send + Sync` on its own. |
| `README.md` | This file. |
| removed | `Cargo.toml.orig`, `Cargo.lock`, `.cargo_vcs_info.json` (packaging metadata cargo does not read for a path dependency). `Cargo.toml` is the published one, unchanged. |

Upstream lets the link's read task and write task use one rustls connection at the same instant. With rustls 0.23.46 and later this breaks TLS links under load (finding F-614). The test is `ops/flashlight/tests/tls_link_concurrent_io.rs` in goatstuff.

## Copies

goatstuff keeps one copy at `vendor/zenoh-link-tls`. A service whose image builds from its own directory keeps an identical copy at `<service>/vendor/zenoh-link-tls`. `ops/ci/check-vendored-zenoh-link-tls.sh` fails if any copy differs from `vendor/zenoh-link-tls`.

## Removal

Delete every copy, and the `[patch.crates-io]` entries that point at them, when a zenoh release whose `zenoh-link-tls` no longer shares the stream between tasks is adopted under the fleet zenoh pin.
